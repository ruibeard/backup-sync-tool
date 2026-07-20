//! Content-defined chunking via FastCDC (v2020).

use fastcdc::v2020::FastCDC;
use sha2::{Digest, Sha256};

/// Target average chunk size (~1 MiB). Boundaries follow content, not fixed offsets.
pub const AVG_CHUNK_SIZE: usize = 1024 * 1024;
pub const MIN_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub sha256_hex: String,
    pub data: Vec<u8>,
}

pub fn chunk_bytes(bytes: &[u8]) -> Vec<Chunk> {
    if bytes.is_empty() {
        return Vec::new();
    }
    // FastCDC returns the remainder as a single chunk when source < min_size.
    FastCDC::new(bytes, MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE)
        .map(|cut| {
            let slice = &bytes[cut.offset..cut.offset + cut.length];
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

    #[test]
    fn identical_bytes_yield_identical_chunks() {
        let mut data = vec![0u8; 3 * 1024 * 1024];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let a = chunk_bytes(&data);
        let b = chunk_bytes(&data);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        assert_eq!(
            a.iter().map(|c| &c.sha256_hex).collect::<Vec<_>>(),
            b.iter().map(|c| &c.sha256_hex).collect::<Vec<_>>()
        );
        let joined: Vec<u8> = a.iter().flat_map(|c| c.data.iter().copied()).collect();
        assert_eq!(joined, data);
    }

    #[test]
    fn large_file_splits_into_multiple_cdc_chunks() {
        // High-entropy payload so gear-hash cut points appear within ~avg size.
        let mut data = vec![0u8; 5 * 1024 * 1024];
        let mut x: u64 = 0xC0FFEE;
        for b in &mut data {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            *b = (x >> 33) as u8;
        }
        let chunks = chunk_bytes(&data);
        assert!(
            chunks.len() >= 2,
            "expected multiple CDC chunks for 5MiB, got {}",
            chunks.len()
        );
        let joined: usize = chunks.iter().map(|c| c.data.len()).sum();
        assert_eq!(joined, data.len());
        for c in &chunks {
            assert!(c.data.len() <= MAX_CHUNK_SIZE);
        }
    }
}
