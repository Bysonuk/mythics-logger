mod common;

use common::{fixture, lines, write_file};
use mythics_logger_core::chunker::{
    self, chunk_path, prepare, prepare_at, ChunkError, Profile, Source, MAX_COMPRESSED,
};
use mythics_logger_core::splitter::{hex, Splitter};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::Path;

fn unpack(dir: &Path, chunks: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for n in 0..chunks {
        let c = std::fs::read(chunk_path(dir, n)).unwrap();
        assert!(c.len() <= MAX_COMPRESSED, "chunk {n} is {} bytes", c.len());
        out.extend(zstd::decode_all(&c[..]).unwrap());
    }
    out
}

#[test]
fn a_pull_compresses_to_chunks_that_unpack_to_the_segment() {
    let bytes = fixture("raid_night.txt");
    let tmp = tempfile::tempdir().unwrap();
    let log = write_file(tmp.path(), "WoWCombatLog.txt", &bytes);
    let mut s = Splitter::new();
    let seg = lines(&bytes)
        .into_iter()
        .find_map(|(o, l)| s.feed(o, l))
        .unwrap();
    let src = Source {
        path: log,
        prefix: seg.prefix(),
        start: seg.start_offset,
        end: seg.end_offset,
    };
    let dir = tmp.path().join("chunks");
    let p = prepare(&src, &dir, Some(&seg.sha256), Profile::Live).unwrap();
    assert_eq!(p.chunks, 1);
    assert_eq!(p.chunk_size, 8 << 20);
    assert_eq!(p.sha256, seg.sha256);
    assert_eq!(p.size, seg.size);
    let raw = unpack(&dir, p.chunks);
    assert!(raw.starts_with(seg.header.as_deref().unwrap().as_bytes()));
    assert_eq!(hex(&Sha256::digest(&raw)), seg.sha256);
}

#[test]
fn a_big_segment_splits_into_several_chunks() {
    // 20 MB of log-like text: three 8 MB pieces, each well under 4 MB packed.
    let tmp = tempfile::tempdir().unwrap();
    let line = b"9/28/2026 20:05:01.5001  SPELL_DAMAGE,Player-1403-0A000001,\"Player1-TarrenMill-EU\",0x512,0x0,Creature-0-1,\"Plexus Sentinel\",0x10a48,0x0,30451,\"Arcane Blast\",0x40\r\n";
    let mut body = Vec::new();
    let mut i = 0u64;
    while body.len() < 20 << 20 {
        body.extend_from_slice(line);
        body.extend_from_slice(format!("{i}\r\n").as_bytes());
        i += 1;
    }
    let log = write_file(tmp.path(), "big.txt", &body);
    let src = Source {
        path: log,
        prefix: Some(b"HEADER\r\n".to_vec()),
        start: 0,
        end: body.len() as u64,
    };
    let dir = tmp.path().join("c");
    let p = prepare(&src, &dir, None, Profile::Live).unwrap();
    assert_eq!(p.chunk_size, 8 << 20);
    assert_eq!(p.chunks, 3);
    let raw = unpack(&dir, p.chunks);
    assert_eq!(raw.len() as u64, src.len());
    assert_eq!(&raw[..8], b"HEADER\r\n");
    assert_eq!(&raw[8..], &body[..]);
}

#[test]
fn text_that_wont_compress_is_cut_smaller_to_fit_4_mb() {
    // Random bytes don't compress: 8 MB and 4 MB pieces overflow 4 MB, so it
    // falls back to 2 MB pieces.
    let tmp = tempfile::tempdir().unwrap();
    let mut body = vec![0u8; 9 << 20];
    rand::thread_rng().fill_bytes(&mut body);
    let log = write_file(tmp.path(), "noise.txt", &body);
    let src = Source {
        path: log,
        prefix: None,
        start: 0,
        end: body.len() as u64,
    };
    let dir = tmp.path().join("c");
    let p = prepare(&src, &dir, None, Profile::Live).unwrap();
    assert_eq!(p.chunk_size, 2 << 20);
    assert_eq!(p.chunks, 5);
    assert_eq!(unpack(&dir, p.chunks), body);
    // No chunk files are left over from the bigger attempts.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 5);
}

#[test]
fn a_file_that_changed_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let log = write_file(tmp.path(), "WoWCombatLog.txt", b"abc\r\n");
    let src = Source {
        path: log,
        prefix: None,
        start: 0,
        end: 5,
    };
    let err = prepare(&src, &tmp.path().join("c"), Some("00"), Profile::Live).unwrap_err();
    assert!(matches!(err, ChunkError::Changed));
    assert!(!tmp.path().join("c").exists());
    assert_eq!(chunker::sha256_of(&src).unwrap().len(), 64);
}

/// Log-like text: the same few events with numbers that change.
fn log_text(bytes: usize) -> Vec<u8> {
    let mut body = Vec::new();
    let mut i = 0u64;
    while body.len() < bytes {
        body.extend_from_slice(
            format!(
                "9/28/2026 20:{:02}:{:02}.{:04}  SPELL_DAMAGE,Player-1403-0A00000{},\"Player{}-TarrenMill-EU\",0x512,0x0,Creature-0-1-2810-1-233814-00001A0001,\"Plexus Sentinel\",0x10a48,0x0,{},\"Arcane Blast\",0x40,Creature-0-1-2810-1-233814-00001A0001,0000000000000000,{},1000000000\r\n",
                (i / 600) % 60,
                (i / 10) % 60,
                i % 10_000,
                i % 7,
                i % 7,
                30451 + i % 13,
                1_000_000_000 - i * 37,
            )
            .as_bytes(),
        );
        i += 1;
    }
    body
}

#[test]
fn a_past_log_compresses_at_level_19_long_and_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let body = log_text(3 << 20);
    let log = write_file(tmp.path(), "past.txt", &body);
    let src = Source {
        path: log,
        prefix: Some(b"HEADER\r\n".to_vec()),
        start: 0,
        end: body.len() as u64,
    };
    let live = tmp.path().join("live");
    let past = tmp.path().join("past");
    let l = prepare(&src, &live, None, Profile::Live).unwrap();
    let p = prepare(&src, &past, None, Profile::Backlog).unwrap();
    // One 32 MB piece holds it all.
    assert_eq!((p.chunk_size, p.chunks), (32 << 20, 1));
    assert_eq!(p.sha256, l.sha256);
    let raw = unpack(&past, p.chunks);
    assert_eq!(&raw[..8], b"HEADER\r\n");
    assert_eq!(&raw[8..], &body[..]);
    let size = |d: &Path, n: u32| -> u64 {
        (0..n)
            .map(|i| std::fs::metadata(chunk_path(d, i)).unwrap().len())
            .sum()
    };
    assert!(
        size(&past, p.chunks) < size(&live, l.chunks),
        "level 19 is smaller than level 10"
    );
    // zstd shrinks the 2^27 window to the chunk: a decoder allowing only
    // 2^22 (4 MiB) reads it, so the server never holds more than it needs.
    let chunk = std::fs::read(chunk_path(&past, 0)).unwrap();
    let mut d = zstd::bulk::Decompressor::new().unwrap();
    d.set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(22))
        .unwrap();
    assert_eq!(d.decompress(&chunk, 4 << 20).unwrap().len(), raw.len());
}

#[test]
fn a_past_logs_chunks_are_compressed_side_by_side_and_come_out_in_order() {
    // At a chunk size the server holds an upload to: 1 MiB, so 3 MiB of log
    // is 4 chunks, compressed two at a time on a bigger computer.
    let tmp = tempfile::tempdir().unwrap();
    let body = log_text(3 << 20);
    let log = write_file(tmp.path(), "past.txt", &body);
    let src = Source {
        path: log,
        prefix: None,
        start: 0,
        end: body.len() as u64,
    };
    let dir = tmp.path().join("c");
    let p = prepare_at(&src, &dir, 1 << 20, None, Profile::Backlog)
        .unwrap()
        .unwrap();
    assert_eq!(p.chunks as usize, body.len().div_ceil(1 << 20));
    for n in 0..p.chunks {
        let one = zstd::decode_all(&std::fs::read(chunk_path(&dir, n)).unwrap()[..]).unwrap();
        let from = n as usize * (1 << 20);
        assert_eq!(&one[..], &body[from..(from + (1 << 20)).min(body.len())]);
    }
    assert!(Profile::Backlog.threads() >= 1 && Profile::Live.threads() == 1);
}
