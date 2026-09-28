//! Content-defined chunking via FastCDC (v2020), streamed from disk.
//!
//! Files are never read whole into memory: the chunker keeps offsets so the
//! uploader can re-read one chunk at a time.

use fastcdc::v2020::StreamCDC;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Target average chunk size (~1 MiB). Boundaries follow content, not fixed offsets.
pub const AVG_CHUNK_SIZE: usize = 1024 * 1024;
pub const MIN_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ChunkRef {
    pub sha256_hex: String,
    pub offset: u64,
    pub len: usize,
}

#[derive(Debug, Clone)]
pub struct FileChunks {
    pub size: u64,
    pub content_sha256: String,
    pub chunks: Vec<ChunkRef>,
}

impl FileChunks {
    pub fn hashes(&self) -> Vec<String> {
        self.chunks.iter().map(|c| c.sha256_hex.clone()).collect()
    }
}

pub fn chunk_file(path: &Path) -> Result<FileChunks, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    chunk_reader(file)
}

pub fn chunk_reader<R: Read>(reader: R) -> Result<FileChunks, String> {
    let mut whole = Sha256::new();
    let mut size = 0u64;
    let mut chunks = Vec::new();
    for chunk in StreamCDC::new(reader, MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE) {
        let chunk = chunk.map_err(|e| format!("chunk read: {e}"))?;
        whole.update(&chunk.data);
        size += chunk.length as u64;
        chunks.push(ChunkRef {
            sha256_hex: hex::encode(Sha256::digest(&chunk.data)),
            offset: chunk.offset,
            len: chunk.length,
        });
    }
    Ok(FileChunks {
        size,
        content_sha256: hex::encode(whole.finalize()),
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy(len: usize) -> Vec<u8> {
        // High-entropy payload so gear-hash cut points appear within ~avg size.
        let mut data = vec![0u8; len];
        let mut x: u64 = 0xC0FFEE;
        for b in &mut data {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            *b = (x >> 33) as u8;
        }
        data
    }

    #[test]
    fn empty_input_has_no_chunks() {
        let out = chunk_reader(&[][..]).unwrap();
        assert!(out.chunks.is_empty());
        assert_eq!(out.size, 0);
        assert_eq!(out.content_sha256, hex::encode(Sha256::digest(b"")));
    }

    #[test]
    fn small_file_is_one_chunk() {
        let out = chunk_reader(&b"hello"[..]).unwrap();
        assert_eq!(out.chunks.len(), 1);
        assert_eq!(out.chunks[0].offset, 0);
        assert_eq!(out.chunks[0].len, 5);
        assert_eq!(out.chunks[0].sha256_hex, out.content_sha256);
    }

    #[test]
    fn large_input_splits_and_covers_every_byte() {
        let data = noisy(5 * 1024 * 1024);
        let out = chunk_reader(&data[..]).unwrap();
        assert!(out.chunks.len() >= 2, "got {} chunks", out.chunks.len());
        assert_eq!(out.size, data.len() as u64);
        assert_eq!(out.content_sha256, hex::encode(Sha256::digest(&data)));
        let mut next = 0u64;
        for c in &out.chunks {
            assert_eq!(c.offset, next);
            assert!(c.len <= MAX_CHUNK_SIZE);
            let slice = &data[c.offset as usize..c.offset as usize + c.len];
            assert_eq!(c.sha256_hex, hex::encode(Sha256::digest(slice)));
            next += c.len as u64;
        }
        assert_eq!(next, data.len() as u64);
    }

    #[test]
    fn local_edit_keeps_most_chunks() {
        let data = noisy(8 * 1024 * 1024);
        let mut edited = data.clone();
        edited[6 * 1024 * 1024] ^= 0xFF;
        let a = chunk_reader(&data[..]).unwrap().hashes();
        let b = chunk_reader(&edited[..]).unwrap().hashes();
        let shared = b.iter().filter(|h| a.contains(h)).count();
        assert!(
            shared + 2 >= b.len(),
            "only {shared} of {} chunks reused",
            b.len()
        );
    }
}
