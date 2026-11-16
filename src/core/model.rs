
#[derive(Clone, Debug)]
pub struct Artifact { pub id: i64, pub path: String, pub kind: String }
#[derive(Clone, Debug)]
pub struct Relation { pub id: i64, pub source: String, pub target: String, pub kind: String, pub evidence: String, pub manual: bool }
#[derive(Clone, Debug)]
pub struct Issue { pub id: i64, pub notebook: String, pub cell: i64, pub filename: String, pub category: String, pub decision: String, pub note: String }
#[derive(Debug)]
pub struct ScanResult { pub files: usize, pub steps: usize, pub links: usize, pub warnings: usize, pub scripts: usize, pub parsed: usize }
