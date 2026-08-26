use async_trait::async_trait;
use phoenix_core::runtime_resource::{RuntimeResourceAdmission, RuntimeResourceAdmissionSink};

use crate::db::Database;

pub(crate) struct DatabaseRuntimeResourceAdmission {
    db: Database,
}

impl DatabaseRuntimeResourceAdmission {
    pub(crate) fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RuntimeResourceAdmissionSink for DatabaseRuntimeResourceAdmission {
    async fn admit_runtime_resource(
        &self,
        admission: RuntimeResourceAdmission,
    ) -> Result<(), String> {
        self.db
            .admit_runtime_resource_instance(admission)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
