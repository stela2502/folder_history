//! Single source of truth for scientific artifact classifications.
use std::{fmt, str::FromStr};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactRole {
    Unknown,
    RawData,
    ProcessedData,
    SampleMetadata,
    ReferenceData,
    ConfigurationParameters,
    ResultFiles,
    ResultsFolder,
    OtherDataFile,
    FiguresImages,
    OtherScientificData,
    NotRelevant,
    AnnotationData,
    StatisticalResults,
    QuantificationResults,
    QualityControlResults,
    ModelResults,
 }

impl ArtifactRole {
    pub const ALL: &'static [Self] = &[
        Self::Unknown,
        Self::RawData,
        Self::ProcessedData,
        Self::SampleMetadata,
        Self::ReferenceData,
        Self::ConfigurationParameters,
        Self::ResultFiles,
        Self::ResultsFolder,
        Self::OtherDataFile,
        Self::FiguresImages,
        Self::OtherScientificData,
        Self::NotRelevant,
        Self::AnnotationData,
        Self::StatisticalResults,
        Self::QuantificationResults,
        Self::QualityControlResults,
        Self::ModelResults,
    ];
    pub fn label(self) -> &'static str { match self {
        Self::Unknown => "Unknown",
        Self::RawData => "Raw experimental data",
        Self::ProcessedData => "Processed data",
        Self::SampleMetadata => "Sample metadata",
        Self::ReferenceData => "Reference data",
        Self::ConfigurationParameters => "Configuration / parameters",
        Self::ResultFiles => "Result files",
        Self::ResultsFolder => "Results folder",
        Self::OtherDataFile => "OtherDataFile",
        Self::FiguresImages => "Figures & images",
        Self::OtherScientificData => "Other scientific data",
        Self::NotRelevant => "Not relevant",
        Self::AnnotationData => "Annotation data",
        Self::StatisticalResults => "Statistical results",
        Self::QuantificationResults => "Quantification results",
        Self::QualityControlResults => "Quality control results",
        Self::ModelResults => "Model results",
    }}
    pub fn yaml_name(self) -> &'static str { match self {
        Self::Unknown => "Unknown",
        Self::RawData => "RawData",
        Self::ProcessedData => "ProcessedData",
        Self::SampleMetadata => "SampleMetadata",
        Self::ReferenceData => "ReferenceData",
        Self::ConfigurationParameters => "ConfigurationParameters",
        Self::ResultFiles => "ResultFiles",
        Self::ResultsFolder => "ResultsFolder",
        Self::OtherDataFile => "OtherDataFile",
        Self::FiguresImages => "FiguresImages",
        Self::OtherScientificData => "OtherScientificData",
        Self::NotRelevant => "NotRelevant",
        Self::AnnotationData => "AnnotationData",
        Self::StatisticalResults => "StatisticalResults",
        Self::QuantificationResults => "QuantificationResults",
        Self::QualityControlResults => "QualityControlResults",
        Self::ModelResults => "ModelResults",
    }}
    pub fn dropdown() -> Vec<Self> {
        let mut roles = Self::ALL.to_vec();
        roles.sort_by_key(|r| (if *r == Self::Unknown { 0 } else { 1 }, r.label().to_ascii_lowercase()));
        roles
    }
}
impl fmt::Display for ArtifactRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.label()) }
}
impl FromStr for ArtifactRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.iter().copied().find(|r| s == r.label() || s == r.yaml_name())
            .ok_or_else(|| format!("Unknown artifact role {s:?}. Accepted: {}", Self::ALL.iter().map(|r| r.yaml_name()).collect::<Vec<_>>().join(", ")))
    }
}
impl Serialize for ArtifactRole {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { serializer.serialize_str(self.yaml_name()) }
}
impl<'de> Deserialize<'de> for ArtifactRole {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn roundtrip_and_dropdown() {
        for role in ArtifactRole::ALL {
            assert_eq!(role.label().parse::<ArtifactRole>().unwrap(), *role);
            assert_eq!(role.yaml_name().parse::<ArtifactRole>().unwrap(), *role);
            assert_eq!(serde_yaml::from_str::<ArtifactRole>(&format!("{}", role.yaml_name())).unwrap(), *role);
        }
        assert!("StatsReslts".parse::<ArtifactRole>().is_err());
        assert_eq!(ArtifactRole::dropdown()[0], ArtifactRole::Unknown);
    }
}
