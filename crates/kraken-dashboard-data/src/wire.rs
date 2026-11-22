//! What the bot pushes to its web backend,
//! [kraken-dashboard-web](https://github.com/crypt0p1a/kraken-dashboard-web): `PUT /state`, a
//! [`Push`] of every screen of the dashboard, as JSON compressed with gzip
//! (`Content-Encoding: gzip`). A screen is mostly series of numbers whose names come again at each
//! point: a quarter of it is left.

use super::{Data, Database, Range, Result};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// The route of the backend the bot pushes to
pub const STATE: &str = "/state";

/// What a screen shows: a range of an account, or of all of them added up when None
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub range: Range,
    pub account: Option<i64>,
    pub data: Data,
}

/// Every screen at once, as the database of the bot was at a time: the backend only keeps the last
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Push {
    /// When the bot read it, by its clock, UTC milliseconds
    pub at: i64,
    pub views: Vec<View>,
}

/// Every screen of the database at `now`: each range of all the accounts added up, then of each
/// account when there are several, the only time the dashboard shows one apart. A view is read at
/// once, the bot may write between two of them
pub fn views(database: &Database, now: i64) -> Result<Vec<View>> {
    let read = |range: Range, account| -> Result<View> {
        let mut data = database.load(range.since(now), now, account)?;
        data.make_finite();
        Ok(View {
            range,
            account,
            data,
        })
    };
    let mut views = Range::ALL
        .into_iter()
        .map(|range| read(range, None))
        .collect::<Result<Vec<_>>>()?;
    let names = &views[0].data.names;
    if names.len() > 1 {
        let accounts: Vec<i64> = names.iter().map(|(id, _)| *id).collect();
        for account in accounts {
            for range in Range::ALL {
                views.push(read(range, Some(account))?);
            }
        }
    }
    Ok(views)
}

/// Compressed, as `Content-Encoding: gzip`
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 4), Compression::default());
    // in memory: writing can't fail
    encoder.write_all(bytes).expect("compressed in memory");
    encoder.finish().expect("compressed in memory")
}

/// What [`gzip`] compressed, its members one after the other, `limit` bytes at most once
/// decompressed: a few bytes can't fill the memory
pub fn gunzip(bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    MultiGzDecoder::new(bytes)
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut decompressed)
        .map_err(|error| format!("not gzip: {error}"))?;
    if decompressed.len() > limit {
        return Err(format!("larger than {limit} bytes once decompressed"));
    }
    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompresses_what_it_compressed_up_to_a_limit() {
        let text = "{\"at\": 1, \"close\": 174.79}".repeat(1_000);
        let compressed = gzip(text.as_bytes());
        assert!(compressed.len() * 10 < text.len());
        assert_eq!(gunzip(&compressed, text.len()).unwrap(), text.as_bytes());
        let error = gunzip(&compressed, text.len() - 1).unwrap_err();
        assert!(error.contains("larger than"), "{error}");
        assert!(gunzip(b"not compressed", 100).is_err());
        // compressed in two parts, as a stream may be
        let mut two = gzip(b"first, ");
        two.extend(gzip(b"second"));
        assert_eq!(gunzip(&two, 100).unwrap(), b"first, second");
        // no limit
        assert_eq!(gunzip(&two, usize::MAX).unwrap(), b"first, second");
    }
}
