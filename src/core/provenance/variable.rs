use super::*;
// Shared symbolic filename representation. Exact strings are also path parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PathPart {
    Literal(String),
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath {
    pub(super) parts: Vec<PathPart>,
}

impl FilePath {
    /// Construct a filename search pattern using the same symbolic path parts
    /// and matching semantics as provenance and YAML expected_files.
    pub fn from_search_pattern(pattern: &str) -> Self {
        let mut parts = Vec::new();
        for (index, fragment) in pattern.split('*').enumerate() {
            if index != 0 { parts.push(PathPart::Wildcard); }
            if !fragment.is_empty() { parts.push(PathPart::Literal(fragment.to_owned())); }
        }
        Self::new(parts)
    }
    pub(super) fn new(parts: Vec<PathPart>) -> Self {
        let mut normalized: Vec<PathPart> = Vec::new();
        for part in parts {
            match part {
                PathPart::Literal(s) if s.is_empty() => {},
                PathPart::Literal(s) => {
                    if let Some(PathPart::Literal(last)) = normalized.last_mut() {
                        last.push_str(&s);
                    } else {
                        normalized.push(PathPart::Literal(s));
                    }
                },
                PathPart::Wildcard => {
                    if !matches!(normalized.last(), Some(PathPart::Wildcard)) {
                        normalized.push(PathPart::Wildcard);
                    }
                },
            }
        }
        Self { parts: normalized }
    }

    /// Reuse the provenance expression parser for an unresolved filename.
    /// A literal anchor is required before suggesting physical candidates.
    pub fn from_issue_expression(expression: &str) -> Option<Self> {
        // Older issue records may have lost the opening quote of a concat.
        let restored;
        let expression = if !expression.starts_with(['"', '\''])
            && expression.contains("\"+") && expression.contains("+\"") {
            restored = format!("\"{expression}");
            restored.as_str()
        } else { expression };
        let path = Variable::from_script_slice(expression, &Environment::default(), "")
            .into_filepath();
        if !path.contains_wildcards() { return None; }
        if !path.parts.iter().any(|part| matches!(part, PathPart::Literal(s)
            if s.chars().any(|c| c.is_alphanumeric()))) { return None; }
        Some(path)
    }

    /// Full relative-path matching for indexed search results.
    pub fn matches_search_path(&self, path: &str) -> bool {
        self.matches(path)
    }

    pub fn matches_filename(&self, physical_path: &str) -> bool {
        let normalized = physical_path.replace('\\', "/");
        self.matches(normalized.rsplit('/').next().unwrap_or(&normalized))
    }

    pub(super) fn contains_wildcards(&self) -> bool {
        self.parts.iter().any(|part| matches!(part, PathPart::Wildcard))
    }

    pub(super) fn matches<S: AsRef<str>>(&self, candidate: S) -> bool {
        let candidate = candidate.as_ref();
        fn match_at(parts: &[PathPart], text: &str) -> bool {
            let Some((first, rest)) = parts.split_first() else { return text.is_empty(); };
            match first {
                PathPart::Literal(literal) => text.strip_prefix(literal)
                    .is_some_and(|remaining| match_at(rest, remaining)),
                PathPart::Wildcard => {
                    // A wildcard matches a single path component, never a slash.
                    // Both slash forms are separators so matching is portable.
                    let component_end = text.find(['/', '\\']).unwrap_or(text.len());
                    (0..=component_end).filter(|&i| text.is_char_boundary(i))
                        .any(|i| match_at(rest, &text[i..]))
                },
            }
        }
        match_at(&self.parts, candidate)
    }

    // Relocation fallback: preserve the basename pattern, discard only the
    // original directory. Reuse the same segment-aware wildcard matcher.
    pub(super) fn funky_match(&self, candidate: &str) -> bool {
        let mut basename = Vec::new();
        for part in &self.parts {
            match part {
                PathPart::Literal(s) => {
                    let s = s.replace('\\', "/");
                    if let Some((_, leaf)) = s.rsplit_once('/') {
                        basename.clear();
                        if !leaf.is_empty() { basename.push(PathPart::Literal(leaf.to_owned())); }
                    } else if !s.is_empty() {
                        basename.push(PathPart::Literal(s));
                    }
                }
                PathPart::Wildcard => basename.push(PathPart::Wildcard),
            }
        }
        let basename = FilePath::new(basename);
        // An unconstrained wildcard is not sufficient relocation evidence.
        if !basename.parts.iter().any(|p| matches!(p, PathPart::Literal(s) if s.chars().any(|c| c.is_alphanumeric()))) {
            return false;
        }
        basename.matches(candidate.replace('\\', "/").rsplit('/').next().unwrap_or(candidate))
    }

    pub(super) fn find_matches(&self, source: &str, paths: &HashSet<String>) -> FileMatches {
        let mut ignored = Vec::new();
        self.find_matches_with_trace(source, paths, &mut ignored)
    }

    /// The diagnostic uses the exact same matching implementation and records
    /// which stages were attempted; no second matching algorithm is involved.
    pub(super) fn find_matches_with_trace(&self, source: &str, paths: &HashSet<String>,
        trace: &mut Vec<String>) -> FileMatches {
        const MAX_CANDIDATES: usize = 50;
        trace.push(format!("search: source={source:?}, indexed candidates={}", paths.len()));
        let dir = source.replace('\\', "/");
        let dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut strict: Vec<String> = paths.iter().filter(|candidate| {
            let local = if dir.is_empty() { Some(candidate.as_str()) }
                else { candidate.strip_prefix(dir).and_then(|s| s.strip_prefix('/')) };
            local.is_some_and(|p| self.matches(p))
        }).cloned().collect();
        strict.sort();
        trace.push(format!("strict: searched paths relative to source directory {dir:?}; matches={}", strict.len()));
        if !strict.is_empty() {
            trace.push("fuzzy: SKIPPED because strict matching succeeded".into());
            return FileMatches::Strict(strict);
        }
        trace.push("fuzzy: ATTEMPTING ancestor-shift relocation (suggestions only)".into());

        // Keep the complete path pattern and move its anchor to any ancestor.
        // Rank by the number of discarded leading directory components: the
        // smallest relocation wins, and no broader stage runs after a hit.
        // This is still only a suggestion, never a confirmed read/write edge.
        let mut by_shift: std::collections::BTreeMap<usize, Vec<String>> =
            std::collections::BTreeMap::new();
        for candidate in paths {
            let candidate = candidate.replace('\\', "/");
            let components: Vec<&str> = candidate.split('/').collect();
            for shift in 1..components.len() {
                if self.matches(components[shift..].join("/")) {
                    by_shift.entry(shift).or_default().push(candidate.clone());
                    break;
                }
            }
        }
        if let Some((shift, mut found)) = by_shift.into_iter().next() {
            trace.push(format!("fuzzy ancestor-shift: shift={shift}, candidates={}", found.len()));
            if found.len() > MAX_CANDIDATES {
                trace.push(format!("fuzzy: REJECTED, more than {MAX_CANDIDATES} candidates"));
                return FileMatches::None;
            }
            found.sort();
            return FileMatches::Funky(found);
        }

        trace.push("fuzzy ancestor-shift: no matches; trying basename relocation".into());
        // Last resort: retain the entire basename pattern, not just extension.
        let mut funky = Vec::new();
        for candidate in paths {
            if self.funky_match(candidate) {
                funky.push(candidate.clone());
                if funky.len() > MAX_CANDIDATES {
                    trace.push(format!("fuzzy basename: REJECTED, more than {MAX_CANDIDATES} candidates"));
                    return FileMatches::None;
                }
            }
        }
        trace.push(format!("fuzzy basename: matches={}", funky.len()));
        if funky.is_empty() { FileMatches::None } else {
            funky.sort();
            FileMatches::Funky(funky)
        }
    }

    pub(super) fn ordered_fragments(&self) -> Vec<String> {
        self.parts.iter().filter_map(|p| match p {
            PathPart::Literal(s) if s.len() >= 4 => Some(s.clone()),
            _ => None,
        }).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FileMatches {
    Strict(Vec<String>),
    Funky(Vec<String>),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Variable {
    Exact(String),
    Parts(FilePath),
    Unknown,
}

impl Variable {
    #[allow(dead_code)] // Used by diagnostic tests; production code calls matches().
    pub(super) fn match_as_str(&self, candidate: &str) -> bool {
        self.matches(candidate)
    }

    pub(super) fn matches<S: AsRef<str>>(&self, candidate: S) -> bool {
        match self {
            Self::Exact(s) => s == candidate.as_ref(),
            Self::Parts(path) => path.matches(candidate),
            Self::Unknown => false,
        }
    }

    pub(super) fn into_filepath(self) -> FilePath {
        match self {
            Self::Exact(s) => FilePath::new(vec![PathPart::Literal(s)]),
            Self::Parts(path) => path,
            Self::Unknown => FilePath::new(vec![PathPart::Wildcard]),
        }
    }

    pub(super) fn from_script_slice(source: &str, env: &Environment, _language: &str) -> Self {
        let source = source.trim();
        // Concatenated filename fragments: preserve exact literal boundaries.
        // Split only at top-level `+`, never inside quotes or indexing/calls.
        // This handles Python, JavaScript and similar expressions without
        // interpreting the dynamic component (including nested quoted keys).
        if let Some(terms) = top_level_concat_terms(source) {
            let mut parts = Vec::new();
            for term in terms {
                if let Some(literal) = quoted_literal(term) {
                    parts.push(PathPart::Literal(literal));
                } else if let Some(exact) = Self::resolve_exact(term, env) {
                    parts.push(PathPart::Literal(exact));
                } else {
                    parts.push(PathPart::Wildcard);
                }
            }
            return Self::Parts(FilePath::new(parts));
        }
        if let Some(exact) = Self::resolve_exact(source, env) {
            return Self::Exact(exact);
        }
        if let Some(existing) = env.variables.get(source) {
            return existing.clone();
        }
        // Any assignment wrapping a quoted wildcard filename pattern can carry
        // symbolic file identity. The function name is deliberately irrelevant:
        // glob.glob("*/*.h5"), custom_loader("*/*.h5"), etc. behave alike.
        // Roles (reads/writes) are still determined by subsequent file operations.
        if let Some(pattern) = wildcard_filename_literal(source) {
            let mut parts = Vec::new();
            for (i, fragment) in pattern.split('*').enumerate() {
                if i != 0 { parts.push(PathPart::Wildcard); }
                if !fragment.is_empty() { parts.push(PathPart::Literal(fragment.to_owned())); }
            }
            return Self::Parts(FilePath::new(parts));
        }
        // Generic dependency: an expression that uses a known symbolic variable
        // carries its file selection forward, independent of the function name.
        // This includes `adatas = [... for f in ifiles]`.
        let identifier = Regex::new(r"[A-Za-z_][A-Za-z_0-9]*").expect("identifier regex");
        let dependencies: Vec<&Variable> = identifier.find_iter(source)
            .filter_map(|m| env.variables.get(m.as_str()))
            .filter(|v| **v != Variable::Unknown)
            .collect();
        if dependencies.len() == 1 {
            return dependencies[0].clone();
        }
        // Preserve literal prefixes and suffixes of unresolved Python f-strings.
        if source.len() >= 3 && (source.starts_with("f\"") || source.starts_with("f'")) {
            let quote = source.as_bytes()[1] as char;
            if source.ends_with(quote) {
                let template = &source[2..source.len()-1];
                let mut parts = Vec::new();
                let mut literal = String::new();
                let mut chars = template.chars().peekable();
                while let Some(ch) = chars.next() {
                    if ch == '{' {
                        if chars.peek() == Some(&'{') {
                            chars.next(); literal.push('{'); continue;
                        }
                        let mut expression = String::new();
                        let mut closed = false;
                        for inner in chars.by_ref() {
                            if inner == '}' { closed = true; break; }
                            expression.push(inner);
                        }
                        if !closed { return Self::Unknown; }
                        if !literal.is_empty() {
                            parts.push(PathPart::Literal(std::mem::take(&mut literal)));
                        }
                        match env.variables.get(expression.trim()) {
                            Some(Variable::Exact(value)) => parts.push(PathPart::Literal(value.clone())),
                            Some(Variable::Parts(path)) => parts.extend(path.parts.iter().cloned()),
                            _ => parts.push(PathPart::Wildcard),
                        }
                    } else if ch == '}' && chars.peek() == Some(&'}') {
                        chars.next(); literal.push('}');
                    } else { literal.push(ch); }
                }
                if !literal.is_empty() { parts.push(PathPart::Literal(literal)); }
                return Self::Parts(FilePath::new(parts));
            }
        }
        // Conservatively extract literal fragments, preserving unknown expressions.
        // Do not mistake option strings such as sep="_" for filename evidence.
        let fragments = ordered_literals(source);
        if fragments.is_empty() { return Self::Unknown; }
        let mut parts = vec![PathPart::Wildcard];
        for fragment in fragments {
            parts.push(PathPart::Literal(fragment));
            parts.push(PathPart::Wildcard);
        }
        Self::Parts(FilePath::new(parts))
    }
}


// Recognize an entire quoted literal, not just a string that happens to
// start and end with quotation marks around an embedded expression.
fn quoted_literal(term: &str) -> Option<String> {
    let term = term.trim();
    let mut chars = term.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '\'' && quote != '"' { return None; }
    let mut escaped = false;
    let mut value = String::new();
    for (idx, ch) in chars {
        if escaped { value.push(ch); escaped = false; continue; }
        if ch == '\\' { escaped = true; continue; }
        if ch == quote {
            return (idx + ch.len_utf8() == term.len()).then_some(value);
        }
        value.push(ch);
    }
    None
}

fn top_level_concat_terms(source: &str) -> Option<Vec<&str>> {
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut terms = Vec::new();
    for (i, ch) in source.char_indices() {
        if escaped { escaped = false; continue; }
        if let Some(q) = quote {
            if ch == '\\' { escaped = true; }
            else if ch == q { quote = None; }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '+' if depth == 0 => {
                terms.push(source[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if terms.is_empty() { return None; }
    terms.push(source[start..].trim());
    if terms.iter().any(|term| term.is_empty()) { return None; }
    // Don't mistake arithmetic for a filename expression.
    if !terms.iter().any(|term| quoted_literal(term).is_some()) { return None; }
    Some(terms)
}

// Extract a wildcard *filename* from any function's quoted arguments, without
// treating arbitrary labels, titles or separator strings as file references.
// Multiple possible patterns are ambiguous and are left to other evidence.
pub(super) fn wildcard_filename_literal(source: &str) -> Option<String> {
    let mut found: Option<String> = None;
    let mut chars = source.chars().peekable();
    while let Some(quote) = chars.next() {
        if quote != '\'' && quote != '"' { continue; }
        let mut value = String::new();
        let mut closed = false;
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(next) = chars.next() { value.push(next); }
            } else if ch == quote { closed = true; break; }
            else { value.push(ch); }
        }
        if !closed { break; }
        // Require a wildcard and a plausible filename extension; no special
        // knowledge of the surrounding function is needed.
        let basename = value.rsplit('/').next().unwrap_or(&value);
        if value.contains('*') && basename.rfind('.').is_some_and(|dot| dot + 1 < basename.len()) {
            if found.is_some() { return None; }
            found = Some(value.replace('\\', "/"));
        }
    }
    found
}

// The single source of truth for variables across assignments and notebook cells.
#[derive(Default)]
pub(super) struct Environment {
    pub(super) variables: HashMap<String, Variable>,
}

impl Environment {
    pub(super) fn assign(&mut self, name: &str, expression: &str, language: &str) {
        let variable = Variable::from_script_slice(expression, self, language);
        self.variables.insert(name.to_owned(), variable);
    }

    pub(super) fn exact(&self, name: &str) -> Option<&str> {
        match self.variables.get(name) {
            Some(Variable::Exact(s)) => Some(s),
            _ => None,
        }
    }
}

