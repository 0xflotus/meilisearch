use meilisearch_types::heed::types::Str;
use meilisearch_types::heed::{Database, Env, RoTxn, RwTxn};
use serde::{Deserialize, Serialize};

use crate::error::FeatureNotEnabledError;
use crate::Result;

const EXPERIMENTAL_FEATURES: &str = "experimental-features";

#[derive(Clone)]
pub(crate) struct FeatureData {
    runtime: Database<Str, Str>,
    instance: InstanceTogglableFeatures,
}

#[derive(Debug, Clone, Copy)]
pub struct RoFeatures {
    runtime: RuntimeTogglableFeatures,
    instance: InstanceTogglableFeatures,
}

impl RoFeatures {
    fn new(txn: RoTxn<'_>, data: &FeatureData) -> Result<Self> {
        let runtime = data.runtime_features(txn)?;
        Ok(Self { runtime, instance: data.instance })
    }

    pub fn runtime_features(&self) -> RuntimeTogglableFeatures {
        self.runtime
    }

    pub fn check_score_details(&self) -> Result<()> {
        if self.runtime.score_details {
            Ok(())
        } else {
            Err(FeatureNotEnabledError {
                disabled_action: "Computing score details",
                feature: "score details",
                issue_link: "https://github.com/meilisearch/product/discussions/674",
            }
            .into())
        }
    }

    pub fn check_metrics(&self) -> Result<()> {
        if self.instance.metrics {
            Ok(())
        } else {
            Err(FeatureNotEnabledError {
                disabled_action: "Getting metrics",
                feature: "metrics",
                issue_link: "https://github.com/meilisearch/meilisearch/discussions/3518",
            }
            .into())
        }
    }

    pub fn check_vector(&self) -> Result<()> {
        if self.runtime.vector_search {
            Ok(())
        } else {
            Err(FeatureNotEnabledError {
                disabled_action: "Using semantic search",
                feature: "semantic search",
                issue_link: "https://github.com/meilisearch/meilisearch/discussions/TODO",
            }
            .into())
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct InstanceTogglableFeatures {
    pub metrics: bool,
}
impl InstanceTogglableFeatures {}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeTogglableFeatures {
    pub score_details: bool,
    pub vector_search: bool,
}

impl FeatureData {
    pub fn new(env: &Env, instance_features: InstanceTogglableFeatures) -> Result<Self> {
        let mut wtxn = env.write_txn()?;
        let runtime_features = env.create_database(&mut wtxn, Some(EXPERIMENTAL_FEATURES))?;
        wtxn.commit()?;

        Ok(Self { runtime: runtime_features, instance: instance_features })
    }

    pub fn put_runtime_features(
        &self,
        mut wtxn: RwTxn,
        features: RuntimeTogglableFeatures,
    ) -> Result<()> {
        self.runtime.put(
            &mut wtxn,
            EXPERIMENTAL_FEATURES,
            // FIXME unwrap
            &serde_json::to_string(&features).unwrap(),
        )?;
        wtxn.commit()?;
        Ok(())
    }

    fn runtime_features(&self, txn: RoTxn<'_>) -> Result<RuntimeTogglableFeatures> {
        let s = self.runtime.get(&txn, EXPERIMENTAL_FEATURES)?.unwrap_or_default();
        Ok(serde_json::from_str(s).unwrap_or_default())
    }

    pub fn features(&self, txn: RoTxn<'_>) -> Result<RoFeatures> {
        RoFeatures::new(txn, self)
    }
}
