//! Fixed-size chunker (CDC FastCDC can replace this later).

use sha2::{Digest, Sha256};

pub const CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub sha256_hex: String,
    pub data: Vec<u8>,
}

pub fn chunk_bytes(bytes: &[u8]) -> Vec<Chunk> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes
        .chunks(CHUNK_SIZE)
        .map(|slice| {
            let mut hasher = Sha256::new();
            hasher.update(slice);
            Chunk {
                sha256_hex: hex::encode(hasher.finalize()),
                data: slice.to_vec(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_has_no_chunks() {
        assert!(chunk_bytes(&[]).is_empty());
    }

    #[test]
    fn small_file_is_one_chunk() {
        let chunks = chunk_bytes(b"hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].data, b"hello");
        assert_eq!(chunks[0].sha256_hex.len(), 64);
    }
}
