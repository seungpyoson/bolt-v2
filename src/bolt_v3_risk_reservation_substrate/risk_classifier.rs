use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskClassifier;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConcentrationBucket {
    bucket_class: String,
    bucket_value: String,
}

impl ConcentrationBucket {
    pub fn new(
        bucket_class: impl Into<String>,
        bucket_value: impl Into<String>,
    ) -> Result<Self, RiskClassificationError> {
        let bucket_class = bucket_class.into();
        let bucket_value = bucket_value.into();
        if !is_clean_runtime_value(&bucket_class) {
            return Err(RiskClassificationError::InvalidBucketClass);
        }
        if !is_clean_runtime_value(&bucket_value) {
            return Err(RiskClassificationError::InvalidBucketValue);
        }
        Ok(Self {
            bucket_class,
            bucket_value,
        })
    }

    pub fn bucket_class(&self) -> &str {
        &self.bucket_class
    }

    pub fn bucket_value(&self) -> &str {
        &self.bucket_value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConcentrationBucketDimension {
    bucket_class: String,
    canonical_attribute: String,
}

impl ConcentrationBucketDimension {
    pub fn new(
        bucket_class: impl Into<String>,
        canonical_attribute: impl Into<String>,
    ) -> Result<Self, RiskClassificationError> {
        let bucket_class = bucket_class.into();
        let canonical_attribute = canonical_attribute.into();
        if !is_clean_runtime_value(&bucket_class) {
            return Err(RiskClassificationError::InvalidBucketClass);
        }
        if !is_clean_runtime_value(&canonical_attribute) {
            return Err(RiskClassificationError::InvalidCanonicalAttribute);
        }
        Ok(Self {
            bucket_class,
            canonical_attribute,
        })
    }

    pub fn bucket_class(&self) -> &str {
        &self.bucket_class
    }

    pub fn canonical_attribute(&self) -> &str {
        &self.canonical_attribute
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskClassificationPolicy {
    required_bucket_dimensions: Vec<ConcentrationBucketDimension>,
}

impl RiskClassificationPolicy {
    pub fn new(
        required_bucket_dimensions: Vec<ConcentrationBucketDimension>,
    ) -> Result<Self, RiskClassificationError> {
        if required_bucket_dimensions.is_empty() {
            return Err(RiskClassificationError::MissingBucketDimensions);
        }
        let mut seen = BTreeSet::new();
        for dimension in &required_bucket_dimensions {
            if !seen.insert((
                dimension.bucket_class.clone(),
                dimension.canonical_attribute.clone(),
            )) {
                return Err(RiskClassificationError::DuplicateBucketDimension {
                    bucket_class: dimension.bucket_class.clone(),
                    canonical_attribute: dimension.canonical_attribute.clone(),
                });
            }
        }
        Ok(Self {
            required_bucket_dimensions,
        })
    }

    pub fn required_bucket_dimensions(&self) -> &[ConcentrationBucketDimension] {
        &self.required_bucket_dimensions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskDescriptorCanonicalAttributes {
    attributes: BTreeMap<String, String>,
}

impl RiskDescriptorCanonicalAttributes {
    pub fn new(attributes: BTreeMap<String, String>) -> Result<Self, RiskClassificationError> {
        for (attribute, value) in &attributes {
            if !is_clean_runtime_value(attribute) {
                return Err(RiskClassificationError::InvalidCanonicalAttribute);
            }
            if !is_clean_runtime_value(value) {
                return Err(RiskClassificationError::InvalidCanonicalAttributeValue);
            }
        }
        Ok(Self { attributes })
    }

    pub fn get(&self, attribute: &str) -> Option<&str> {
        self.attributes.get(attribute).map(String::as_str)
    }

    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskClassification {
    buckets: BTreeSet<ConcentrationBucket>,
    diagnostic_caller_declared_buckets: Vec<ConcentrationBucket>,
}

impl RiskClassification {
    pub fn buckets(&self) -> &BTreeSet<ConcentrationBucket> {
        &self.buckets
    }

    pub fn diagnostic_caller_declared_buckets(&self) -> &[ConcentrationBucket] {
        &self.diagnostic_caller_declared_buckets
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskClassificationError {
    InvalidBucketClass,
    InvalidBucketValue,
    InvalidCanonicalAttribute,
    InvalidCanonicalAttributeValue,
    MissingBucketDimensions,
    DuplicateBucketDimension {
        bucket_class: String,
        canonical_attribute: String,
    },
    MissingCanonicalAttribute {
        attribute: String,
    },
}

impl RiskClassifier {
    pub fn classify(
        descriptor: &RiskDescriptorCanonicalAttributes,
        policy: &RiskClassificationPolicy,
        caller_declared_buckets: &[ConcentrationBucket],
    ) -> Result<RiskClassification, RiskClassificationError> {
        let mut buckets = BTreeSet::new();
        for dimension in policy.required_bucket_dimensions() {
            let bucket_value =
                descriptor
                    .get(dimension.canonical_attribute())
                    .ok_or_else(|| RiskClassificationError::MissingCanonicalAttribute {
                        attribute: dimension.canonical_attribute().to_string(),
                    })?;
            buckets.insert(ConcentrationBucket::new(
                dimension.bucket_class(),
                bucket_value,
            )?);
        }
        Ok(RiskClassification {
            buckets,
            diagnostic_caller_declared_buckets: caller_declared_buckets.to_vec(),
        })
    }
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
