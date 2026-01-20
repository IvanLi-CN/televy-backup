use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifest {
    pub version: u8,
    pub snapshot_id: String,
    pub hash_alg: String,
    pub enc_alg: String,
    pub compression: String,
    pub parts: Vec<IndexManifestPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifestPart {
    pub no: u32,
    pub size: usize,
    pub hash: String,
    pub object_id: String,
}

pub fn index_part_aad(snapshot_id: &str, part_no: u32) -> String {
    format!("{snapshot_id}:{part_no}")
}
