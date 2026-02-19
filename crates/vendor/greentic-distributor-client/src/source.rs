use crate::{ComponentId, DistributorError, PackId, Version};

/// Pluggable source for fetching packs and components by identifier/version.
pub trait DistributorSource: Send + Sync {
    fn fetch_pack(&self, pack_id: &PackId, version: &Version) -> Result<Vec<u8>, DistributorError>;

    fn fetch_component(
        &self,
        component_id: &ComponentId,
        version: &Version,
    ) -> Result<Vec<u8>, DistributorError>;
}

/// Simple priority-ordered collection of sources that tries each until one succeeds.
pub struct ChainedDistributorSource {
    sources: Vec<Box<dyn DistributorSource>>,
}

impl ChainedDistributorSource {
    pub fn new(sources: Vec<Box<dyn DistributorSource>>) -> Self {
        Self { sources }
    }
}

impl DistributorSource for ChainedDistributorSource {
    fn fetch_pack(&self, pack_id: &PackId, version: &Version) -> Result<Vec<u8>, DistributorError> {
        for source in &self.sources {
            match source.fetch_pack(pack_id, version) {
                Ok(bytes) => return Ok(bytes),
                Err(DistributorError::NotFound) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(DistributorError::NotFound)
    }

    fn fetch_component(
        &self,
        component_id: &ComponentId,
        version: &Version,
    ) -> Result<Vec<u8>, DistributorError> {
        for source in &self.sources {
            match source.fetch_component(component_id, version) {
                Ok(bytes) => return Ok(bytes),
                Err(DistributorError::NotFound) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(DistributorError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MemorySource {
        packs: HashMap<(PackId, Version), Vec<u8>>,
        components: HashMap<(ComponentId, Version), Vec<u8>>,
        error: Option<String>,
    }

    impl MemorySource {
        fn new() -> Self {
            Self {
                packs: HashMap::new(),
                components: HashMap::new(),
                error: None,
            }
        }

        fn with_error(err: impl Into<String>) -> Self {
            Self {
                packs: HashMap::new(),
                components: HashMap::new(),
                error: Some(err.into()),
            }
        }
    }

    impl DistributorSource for MemorySource {
        fn fetch_pack(
            &self,
            pack_id: &PackId,
            version: &Version,
        ) -> Result<Vec<u8>, DistributorError> {
            if let Some(err) = &self.error {
                return Err(DistributorError::Other(err.clone()));
            }
            self.packs
                .get(&(pack_id.clone(), version.clone()))
                .cloned()
                .ok_or(DistributorError::NotFound)
        }

        fn fetch_component(
            &self,
            component_id: &ComponentId,
            version: &Version,
        ) -> Result<Vec<u8>, DistributorError> {
            if let Some(err) = &self.error {
                return Err(DistributorError::Other(err.clone()));
            }
            self.components
                .get(&(component_id.clone(), version.clone()))
                .cloned()
                .ok_or(DistributorError::NotFound)
        }
    }

    #[test]
    fn chained_prefers_first_success() {
        let version = Version::parse("1.0.0").unwrap();
        let pack_id = PackId::try_from("pack.one").unwrap();
        let mut primary = MemorySource::new();
        primary
            .packs
            .insert((pack_id.clone(), version.clone()), b"pack".to_vec());
        let chained =
            ChainedDistributorSource::new(vec![Box::new(primary), Box::new(MemorySource::new())]);

        let bytes = chained.fetch_pack(&pack_id, &version).unwrap();
        assert_eq!(bytes, b"pack");
    }

    #[test]
    fn chained_skips_not_found_continues() {
        let version = Version::parse("1.0.0").unwrap();
        let pack_id = PackId::try_from("pack.missing").unwrap();
        let mut fallback = MemorySource::new();
        fallback
            .packs
            .insert((pack_id.clone(), version.clone()), b"found".to_vec());
        let chained =
            ChainedDistributorSource::new(vec![Box::new(MemorySource::new()), Box::new(fallback)]);

        let bytes = chained.fetch_pack(&pack_id, &version).unwrap();
        assert_eq!(bytes, b"found");
    }

    #[test]
    fn chained_propagates_other_errors() {
        let version = Version::parse("1.0.0").unwrap();
        let pack_id = PackId::try_from("pack.error").unwrap();
        let chained =
            ChainedDistributorSource::new(vec![Box::new(MemorySource::with_error("boom"))]);

        let err = chained.fetch_pack(&pack_id, &version).unwrap_err();
        assert!(matches!(err, DistributorError::Other(msg) if msg == "boom"));
    }
}
