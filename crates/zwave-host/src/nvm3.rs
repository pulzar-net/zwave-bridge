//! NVM3, the flash file system of 700 and 800 series controllers.
//!
//! An image is made of pages (a header with a Berger-coded erase count,
//! then word-aligned objects with Berger-coded headers). Objects are keyed
//! files; a file may be split into fragments; the newest copy wins. A 700
//! series image holds two volumes (application, then protocol); an 800
//! series image one shared volume.
//!
//! This module reads images (to identify a backup before restoring it) and
//! writes them (for tests and simulators, and later for conversion between
//! series).

use std::collections::HashMap;

const PAGE_HEADER: usize = 20;
const PAGE_MAGIC: u16 = 0xb29a;
const MIN_PAGE: usize = 512;
const COUNTER_BITS: u32 = 27;
const COUNTER_MASK: u32 = (1 << COUNTER_BITS) - 1;
const KEY_SHIFT: u32 = 7;
const KEY_MASK: u32 = (1 << 20) - 1;
const TYPE_MASK: u32 = 0x7f;
const LARGE_LEN_MASK: u32 = (1 << 26) - 1;
const COUNTER_SMALL_SIZE: usize = 204;

/// The application volume's size in a 700 series image.
pub const APPLICATION_SIZE_700: usize = 0x3000;
/// A 700 series image (application and protocol volumes).
pub const IMAGE_SIZE_700: usize = 0xc000;
/// An 800 series image (one shared volume).
pub const IMAGE_SIZE_800: usize = 0xa000;

/// Protocol version file.
pub const PROTOCOL_VERSION_FILE: u32 = 0x50000;
/// Controller information file (home id, node id).
pub const CONTROLLER_INFO_FILE: u32 = 0x50004;
/// Application version file, 700 series.
pub const APPLICATION_VERSION_FILE_700: u32 = 0x51000;
/// Application version file, 800 series (marks a shared volume).
pub const APPLICATION_VERSION_FILE_800: u32 = 0x41000;

/// Why an image could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("NVM3 at {offset:#x}: {message}")]
pub struct Nvm3Error {
    /// Where.
    pub offset: usize,
    /// What.
    pub message: &'static str,
}

fn bad(offset: usize, message: &'static str) -> Nvm3Error {
    Nvm3Error { offset, message }
}

/// The Berger code of the low `bits` bits of `word`: the number of zeros.
fn berger(word: u32, bits: u32) -> u32 {
    let w = if bits < 32 {
        word & ((1 << bits) - 1)
    } else {
        word
    };
    bits - w.count_ones()
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frag {
    None,
    First,
    Next,
    Last,
}

#[derive(Debug, Clone)]
struct Object {
    key: u32,
    deleted: bool,
    frag: Frag,
    data: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct Page {
    erase_count: u32,
    objects: Vec<Object>,
}

/// A parsed image.
#[derive(Debug, Clone)]
pub struct Nvm3 {
    volumes: Vec<Vec<Page>>,
    /// One shared volume (800 series) rather than two (700 series).
    pub shared: bool,
    /// The page size.
    pub page_size: usize,
    files: HashMap<u32, Vec<u8>>,
}

fn parse_page(image: &[u8], offset: usize) -> Result<(Page, usize), Nvm3Error> {
    let h = image
        .get(offset..offset + PAGE_HEADER)
        .ok_or(bad(offset, "incomplete page header"))?;
    let version = u16::from_le_bytes([h[0], h[1]]);
    let magic = u16::from_le_bytes([h[2], h[3]]);
    if magic != PAGE_MAGIC {
        return Err(bad(offset, "not an NVM3 page"));
    }
    if version != 1 {
        return Err(bad(offset, "unsupported NVM3 page version"));
    }
    let ec = u32le(h, 4).expect("in header");
    let eci = u32le(h, 8).expect("in header");
    let (count, inv) = (ec & COUNTER_MASK, eci & COUNTER_MASK);
    if berger(count, COUNTER_BITS) != ec >> COUNTER_BITS
        || berger(inv, COUNTER_BITS) != eci >> COUNTER_BITS
        || count != (!inv & COUNTER_MASK)
    {
        return Err(bad(offset, "invalid erase count"));
    }
    let dev_info = u16::from_le_bytes([h[16], h[17]]);
    let mut size = MIN_PAGE << ((dev_info >> 13) & 0b111);
    // Some images leave the page size bits unprogrammed: find the next valid header instead.
    if (dev_info >> 13) & 0b111 == 0b111 {
        size = (0..7)
            .map(|e| MIN_PAGE << e)
            .find(|&s| offset + s == image.len() || parse_header_ok(image, offset + s))
            .ok_or(bad(offset, "cannot determine the page size"))?;
    }
    if offset + size > image.len() {
        return Err(bad(offset, "incomplete page"));
    }
    let mut objects = Vec::new();
    let mut at = offset + PAGE_HEADER;
    let end = offset + size;
    while at + 4 <= end {
        let hdr1 = u32le(image, at).expect("bounded");
        if hdr1 == u32::MAX {
            break;
        }
        let key = (hdr1 >> KEY_SHIFT) & KEY_MASK;
        let mut typ = hdr1 & TYPE_MASK;
        let large = typ == 0 || typ == 1;
        let (header, len, frag) = if large {
            let hdr2 = u32le(image, at + 4).ok_or(bad(at, "incomplete object header"))?;
            let code = berger(hdr1, 32) + berger(hdr2, 26);
            if code != hdr2 >> 26 {
                return Err(bad(at, "object header check failed"));
            }
            let frag = match (hdr1 >> 27) & 0b11 {
                0 => Frag::None,
                1 => Frag::First,
                2 => Frag::Next,
                _ => Frag::Last,
            };
            (8, (hdr2 & LARGE_LEN_MASK) as usize, frag)
        } else {
            if berger(hdr1, 27) != hdr1 >> 27 {
                return Err(bad(at, "object header check failed"));
            }
            let len = if typ > 7 {
                let l = (typ - 7) as usize;
                typ = 7;
                l
            } else if typ == 2 {
                COUNTER_SMALL_SIZE
            } else {
                0
            };
            (4, len, Frag::None)
        };
        let data = at + header..at + header + len;
        if data.end > end {
            return Err(bad(at, "object runs past its page"));
        }
        objects.push(Object {
            key,
            deleted: typ == 3,
            frag,
            data,
        });
        at += header + len.div_ceil(4) * 4;
    }
    Ok((
        Page {
            erase_count: count,
            objects,
        },
        size,
    ))
}

fn parse_header_ok(image: &[u8], offset: usize) -> bool {
    let Some(h) = image.get(offset..offset + PAGE_HEADER) else {
        return false;
    };
    u16::from_le_bytes([h[2], h[3]]) == PAGE_MAGIC && u16::from_le_bytes([h[0], h[1]]) == 1
}

/// The newest complete copy of `key` in a volume, reading pages from the
/// current one backwards and each page's objects from the end.
fn find(image: &[u8], pages: &[Page], key: u32) -> Option<Vec<u8>> {
    if pages.is_empty() {
        return None;
    }
    let max = pages.iter().map(|p| p.erase_count).max()?;
    let current = pages
        .iter()
        .rposition(|p| p.erase_count == max && !p.objects.is_empty())
        .or_else(|| pages.iter().rposition(|p| !p.objects.is_empty()))
        .unwrap_or(0);
    let mut parts: Vec<&[u8]> = Vec::new();
    for k in 0..pages.len() {
        let page = &pages[(current + pages.len() - k) % pages.len()];
        for o in page.objects.iter().rev() {
            if o.key != key {
                parts.clear();
                continue;
            }
            if o.deleted {
                return None;
            }
            let d = &image[o.data.clone()];
            match o.frag {
                Frag::None => return Some(d.to_vec()),
                Frag::Last => parts = vec![d],
                Frag::Next if !parts.is_empty() => parts.insert(0, d),
                Frag::First if !parts.is_empty() => {
                    parts.insert(0, d);
                    return Some(parts.concat());
                }
                _ => parts.clear(),
            }
        }
    }
    None
}

impl Nvm3 {
    /// Parses an image.
    pub fn parse(image: &[u8]) -> Result<Nvm3, Nvm3Error> {
        let mut pages = Vec::new();
        let mut offsets = Vec::new();
        let mut at = 0;
        let mut page_size = 0;
        while at < image.len() {
            let (p, size) = parse_page(image, at)?;
            page_size = page_size.max(size);
            offsets.push(at);
            pages.push(p);
            at += size;
        }
        let shared = pages.iter().any(|p| {
            p.objects
                .iter()
                .any(|o| o.key == APPLICATION_VERSION_FILE_800)
        });
        let volumes = if shared {
            vec![pages]
        } else {
            let (app, proto): (Vec<_>, Vec<_>) = pages
                .into_iter()
                .zip(offsets)
                .partition(|(_, o)| *o < APPLICATION_SIZE_700);
            vec![
                app.into_iter().map(|x| x.0).collect(),
                proto.into_iter().map(|x| x.0).collect(),
            ]
        };
        let mut keys: Vec<u32> = volumes
            .iter()
            .flatten()
            .flat_map(|p| p.objects.iter().map(|o| o.key))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let mut files = HashMap::new();
        for k in keys {
            for v in &volumes {
                if let Some(d) = find(image, v, k) {
                    files.insert(k, d);
                    break;
                }
            }
        }
        Ok(Nvm3 {
            volumes,
            shared,
            page_size,
            files,
        })
    }

    /// A file's current contents.
    pub fn file(&self, key: u32) -> Option<&[u8]> {
        self.files.get(&key).map(Vec::as_slice)
    }

    /// Every file key present.
    pub fn keys(&self) -> Vec<u32> {
        let mut k: Vec<u32> = self.files.keys().copied().collect();
        k.sort_unstable();
        k
    }

    /// Pages per volume.
    pub fn pages(&self) -> Vec<usize> {
        self.volumes.iter().map(Vec::len).collect()
    }
}

/// A version file: `major.minor.patch` and the file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    /// Major.
    pub major: u8,
    /// Minor.
    pub minor: u8,
    /// Patch.
    pub patch: u8,
    /// File system format.
    pub format: u8,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn version(d: &[u8]) -> Option<Version> {
    (d.len() >= 4).then(|| Version {
        patch: d[0],
        minor: d[1],
        major: d[2],
        format: d[3],
    })
}

/// What a controller backup says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvmInfo {
    /// 800 series layout (one shared volume).
    pub shared: bool,
    /// Image size.
    pub size: usize,
    /// Protocol version and file format (700 series images only).
    pub protocol: Option<Version>,
    /// Application version.
    pub application: Option<Version>,
    /// Home id.
    pub home_id: Option<u32>,
    /// The controller's node id.
    pub node_id: Option<u16>,
}

impl NvmInfo {
    /// Reads an image's identity.
    pub fn read(image: &[u8]) -> Result<NvmInfo, Nvm3Error> {
        let n = Nvm3::parse(image)?;
        let ci = n.file(CONTROLLER_INFO_FILE);
        // The shared (800 series) layout has no protocol version file.
        let protocol = n.file(PROTOCOL_VERSION_FILE).and_then(version);
        let lr_format = n.shared || protocol.is_some_and(|p| p.format >= 5);
        Ok(NvmInfo {
            shared: n.shared,
            size: image.len(),
            protocol,
            application: n
                .file(APPLICATION_VERSION_FILE_800)
                .or(n.file(APPLICATION_VERSION_FILE_700))
                .and_then(version),
            home_id: ci
                .and_then(|c| c.get(0..4))
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            node_id: ci.and_then(|c| {
                if lr_format {
                    c.get(4..6).map(|b| u16::from_le_bytes([b[0], b[1]]))
                } else {
                    c.get(4).map(|&b| u16::from(b))
                }
            }),
        })
    }

    /// Whether `self` (a backup) can be written as it is onto a controller whose current image is `target`.
    pub fn restorable_onto(&self, target: &NvmInfo) -> Result<(), String> {
        if self.size != target.size || self.shared != target.shared {
            return Err(format!(
                "the backup is a {} image of {} bytes, the controller a {} image of {} bytes; that needs conversion between series",
                if self.shared { "800 series" } else { "700 series" },
                self.size,
                if target.shared { "800 series" } else { "700 series" },
                target.size
            ));
        }
        if self.shared {
            // 800 series images carry the controller application's version, not the SDK's.
            return match (self.application, target.application) {
                (Some(a), Some(b)) if a == b => Ok(()),
                (Some(a), Some(b)) => Err(format!("the backup is from controller firmware {a}, the controller runs {b}; that needs conversion")),
                _ => Err("an image has no application version file".into()),
            };
        }
        match (self.protocol, target.protocol) {
            (Some(a), Some(b)) if a.format != b.format || (a.major, a.minor) != (b.major, b.minor) => Err(format!(
                "the backup is from protocol {a} (format {}), the controller runs {b} (format {}); that needs conversion",
                a.format, b.format
            )),
            (None, _) | (_, None) => Err("an image has no protocol version file".into()),
            _ => Ok(()),
        }
    }
}

// ----------------------------------------------------------------- writing

fn page_header(erase_count: u32, page_size: usize) -> [u8; PAGE_HEADER] {
    let mut h = [0u8; PAGE_HEADER];
    h[0..2].copy_from_slice(&1u16.to_le_bytes());
    h[2..4].copy_from_slice(&PAGE_MAGIC.to_le_bytes());
    let ec = erase_count & COUNTER_MASK;
    let inv = !erase_count & COUNTER_MASK;
    h[4..8].copy_from_slice(&(ec | berger(ec, COUNTER_BITS) << COUNTER_BITS).to_le_bytes());
    h[8..12].copy_from_slice(&(inv | berger(inv, COUNTER_BITS) << COUNTER_BITS).to_le_bytes());
    h[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    let bits = (page_size / MIN_PAGE).trailing_zeros() as u16;
    h[16..18].copy_from_slice(&((bits << 13) | (1 << 12)).to_le_bytes());
    h[18..20].copy_from_slice(&0xffffu16.to_le_bytes());
    h
}

fn object(key: u32, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    if data.len() <= 120 {
        let hdr = ((7 + data.len() as u32) & TYPE_MASK) | ((key & KEY_MASK) << KEY_SHIFT);
        v.extend_from_slice(&(hdr | berger(hdr, 27) << 27).to_le_bytes());
    } else {
        let hdr1 = (key & KEY_MASK) << KEY_SHIFT;
        let hdr2 = data.len() as u32 & LARGE_LEN_MASK;
        let code = berger(hdr1, 32) + berger(hdr2, 26);
        v.extend_from_slice(&hdr1.to_le_bytes());
        v.extend_from_slice(&(hdr2 | code << 26).to_le_bytes());
    }
    v.extend_from_slice(data);
    v.resize(v.len().div_ceil(4) * 4, 0xff);
    v
}

/// Writes a volume of `size` bytes holding `files`, packed from the first
/// page, the rest erased. Files must fit one page each.
pub fn write_volume(
    size: usize,
    page_size: usize,
    files: &[(u32, Vec<u8>)],
) -> Result<Vec<u8>, &'static str> {
    if page_size < MIN_PAGE || !size.is_multiple_of(page_size) {
        return Err("bad volume geometry");
    }
    let mut out = vec![0xffu8; size];
    let mut page = 0;
    let mut at = PAGE_HEADER;
    for (key, data) in files {
        let obj = object(*key, data);
        if at + obj.len() > page_size {
            page += 1;
            at = PAGE_HEADER;
            if (page + 1) * page_size > size || obj.len() + PAGE_HEADER > page_size {
                return Err("files do not fit the volume");
            }
        }
        out[page * page_size + at..page * page_size + at + obj.len()].copy_from_slice(&obj);
        at += obj.len();
    }
    for p in 0..size / page_size {
        out[p * page_size..p * page_size + PAGE_HEADER].copy_from_slice(&page_header(1, page_size));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(major: u8, minor: u8, patch: u8, format: u8) -> Vec<u8> {
        vec![patch, minor, major, format]
    }

    #[test]
    fn write_then_read_700_and_800() {
        let mut ci = vec![0xc0, 0xff, 0xee, 0x01, 1];
        ci.resize(13, 0);
        let big: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let app = write_volume(
            APPLICATION_SIZE_700,
            2048,
            &[(APPLICATION_VERSION_FILE_700, ver(7, 18, 1, 0))],
        )
        .unwrap();
        let proto = write_volume(
            IMAGE_SIZE_700 - APPLICATION_SIZE_700,
            2048,
            &[
                (PROTOCOL_VERSION_FILE, ver(7, 17, 0, 3)),
                (CONTROLLER_INFO_FILE, ci),
                (0x50005, big.clone()),
            ],
        )
        .unwrap();
        let image = [app, proto].concat();
        let n = Nvm3::parse(&image).unwrap();
        assert!(!n.shared);
        assert_eq!(n.file(0x50005), Some(&big[..]));
        let info = NvmInfo::read(&image).unwrap();
        assert_eq!(info.home_id, Some(0xc0ffee01));
        assert_eq!(info.node_id, Some(1));
        assert_eq!(info.protocol.unwrap().to_string(), "7.17.0");

        let mut ci = vec![0xc0, 0xff, 0xee, 0x02, 1, 0];
        ci.resize(22, 0);
        let shared = write_volume(
            IMAGE_SIZE_800,
            8192,
            &[
                (APPLICATION_VERSION_FILE_800, ver(7, 23, 0, 0)),
                (PROTOCOL_VERSION_FILE, ver(7, 23, 0, 5)),
                (CONTROLLER_INFO_FILE, ci),
            ],
        )
        .unwrap();
        let info8 = NvmInfo::read(&shared).unwrap();
        assert!(info8.shared);
        assert_eq!((info8.home_id, info8.node_id), (Some(0xc0ffee02), Some(1)));
        assert!(info.restorable_onto(&info).is_ok());
        assert!(info
            .restorable_onto(&info8)
            .unwrap_err()
            .contains("conversion"));
    }

    #[test]
    fn corrupt_images_are_refused() {
        let mut v = write_volume(4096, 2048, &[(PROTOCOL_VERSION_FILE, ver(7, 17, 0, 3))]).unwrap();
        assert!(Nvm3::parse(&v).is_ok());
        v[PAGE_HEADER] ^= 0x01;
        assert!(Nvm3::parse(&v).is_err(), "object header check");
        assert!(Nvm3::parse(&[0u8; 100]).is_err());
        assert!(Nvm3::parse(&[]).is_ok_and(|n| n.keys().is_empty()));
    }
}
