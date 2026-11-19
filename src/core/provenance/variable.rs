use super::*;
// Shared symbolic filename representation. Exact strings are also path parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PathPart {
    Literal(String),
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FilePath {
    pub(super) parts: Vec<PathPart>,
}

impl FilePath {
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
        const MAX_CANDIDATES: usize = 50;
        let dir = source.replace('\\', "/");
        let dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut strict: Vec<String> = paths.iter().filter(|candidate| {
            let local = if dir.is_empty() { Some(candidate.as_str()) }
                else { candidate.strip_prefix(dir).and_then(|s| s.strip_prefix('/')) };
            local.is_some_and(|p| self.matches(p))
        }).cloned().collect();
        strict.sort();
        if !strict.is_empty() { return FileMatches::Strict(strict); }

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
        if let Some((_, mut found)) = by_shift.into_iter().next() {
            if found.len() > MAX_CANDIDATES { return FileMatches::None; }
            found.sort();
            return FileMatches::Funky(found);
        }

        // Last resort: retain the entire basename pattern, not just extension.
        let mut funky = Vec::new();
        for candidate in paths {
            if self.funky_match(candidate) {
                funky.push(candidate.clone());
                if funky.len() > MAX_CANDIDATES { return FileMatches::None; }
            }
        }
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

