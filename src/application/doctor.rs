use crate::application::repository::{KnowledgeGraphRepository, RepositoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorReport {
    schema_version: u64,
}

impl DoctorReport {
    pub fn schema_version(&self) -> u64 {
        self.schema_version
    }
}

pub struct DoctorService;

impl DoctorService {
    pub fn execute(
        &self,
        repository: &impl KnowledgeGraphRepository,
    ) -> Result<DoctorReport, RepositoryError> {
        Ok(DoctorReport {
            schema_version: repository.schema_version()?,
        })
    }
}
