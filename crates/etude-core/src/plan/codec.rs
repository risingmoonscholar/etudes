use super::{BindingContext, Group, Plan, Signal};
use crate::scan::{SnapshotEntry, SnapshotIdentity, TreeSnapshot};
use crate::{Category, Untouched};
use std::{
    io,
    path::{Path, PathBuf},
};

const MAGIC: &[u8] = b"ETUDEPLAN\0\x01";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or unsupported plan binding",
    )
}
struct Writer(Vec<u8>);
impl Writer {
    fn byte(&mut self, v: u8) {
        self.0.push(v);
    }
    fn n(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    fn count(&mut self, v: usize) -> io::Result<()> {
        if v > MAX_ROWS {
            return Err(invalid());
        }
        self.n(v as u64);
        Ok(())
    }
    fn bytes(&mut self, v: &[u8]) -> io::Result<()> {
        if v.len() > 4096
            || self
                .0
                .len()
                .checked_add(v.len() + 8)
                .is_none_or(|size| size > MAX_BYTES)
        {
            return Err(invalid());
        }
        self.n(v.len() as u64);
        self.0.extend(v);
        Ok(())
    }
    fn text(&mut self, v: &str) -> io::Result<()> {
        self.bytes(v.as_bytes())
    }
    fn path(&mut self, v: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            self.bytes(v.as_os_str().as_bytes())
        }
        #[cfg(not(unix))]
        {
            self.text(v.to_str().ok_or_else(invalid)?)
        }
    }
    fn identity(&mut self, i: &SnapshotIdentity) {
        for n in [
            i.device,
            i.inode,
            i.size,
            i.mtime_sec as u64,
            i.mtime_nsec as u64,
            i.ctime_sec as u64,
            i.ctime_nsec as u64,
            u64::from(i.mode),
        ] {
            self.n(n);
        }
        self.byte(i.kind);
    }
    fn entries(&mut self, entries: &[SnapshotEntry]) -> io::Result<()> {
        self.count(entries.len())?;
        for entry in entries {
            self.path(&TreeSnapshot::path_commitment(&entry.path))?;
            self.identity(&entry.identity);
        }
        Ok(())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(invalid)?;
        let bytes = self.bytes.get(self.at..end).ok_or_else(invalid)?;
        self.at = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn n(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| invalid())?,
        ))
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = usize::try_from(self.n()?).map_err(|_| invalid())?;
        if n > MAX_ROWS || n > self.bytes.len() - self.at {
            return Err(invalid());
        }
        Ok(n)
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = usize::try_from(self.n()?).map_err(|_| invalid())?;
        if n > 4096 {
            return Err(invalid());
        }
        self.take(n)
    }
    fn text(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| invalid())
    }
    fn path(&mut self) -> io::Result<PathBuf> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(self.bytes()?.to_vec()).into())
        }
        #[cfg(not(unix))]
        {
            Ok(self.text()?.into())
        }
    }
    fn boolean(&mut self) -> io::Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
    fn usize(&mut self) -> io::Result<usize> {
        usize::try_from(self.n()?).map_err(|_| invalid())
    }
    fn identity(&mut self) -> io::Result<SnapshotIdentity> {
        let i = SnapshotIdentity {
            device: self.n()?,
            inode: self.n()?,
            size: self.n()?,
            mtime_sec: self.n()? as i64,
            mtime_nsec: self.n()? as i64,
            ctime_sec: self.n()? as i64,
            ctime_nsec: self.n()? as i64,
            mode: u32::try_from(self.n()?).map_err(|_| invalid())?,
            kind: self.byte()?,
        };
        if i.kind == 0 {
            if i.device != 0
                || i.inode != 0
                || i.size != 0
                || i.mtime_sec != 0
                || i.mtime_nsec != 0
                || i.ctime_sec != 0
                || i.ctime_nsec != 0
                || i.mode != 0
            {
                return Err(invalid());
            }
            return Ok(i);
        }
        if !(1..=4).contains(&i.kind)
            || !(0..1_000_000_000).contains(&i.mtime_nsec)
            || !(0..1_000_000_000).contains(&i.ctime_nsec)
        {
            return Err(invalid());
        }
        Ok(i)
    }
    fn entries(&mut self) -> io::Result<Vec<SnapshotEntry>> {
        let count = self.count()?;
        let mut entries = Vec::new();
        for _ in 0..count {
            entries.push(SnapshotEntry {
                path: self.path()?,
                identity: self.identity()?,
            });
        }
        Ok(entries)
    }
}

pub(super) fn encode(
    plan: &Plan,
    snapshot: &TreeSnapshot,
    context: &BindingContext,
) -> io::Result<Vec<u8>> {
    let mut w = Writer(MAGIC.to_vec());
    for value in [
        &context.tool,
        &context.tool_version,
        &context.profile,
        &context.observation_contract,
    ] {
        w.text(value)?;
    }
    w.path(&plan.root)?;
    w.count(plan.groups.len())?;
    let mut total = 0usize;
    for group in &plan.groups {
        w.text(&group.name)?;
        match &group.signal {
            Signal::Screenshot => w.byte(0),
            Signal::CameraBurst { days } => {
                w.byte(1);
                w.n(u64::from(*days));
            }
            Signal::Installer => w.byte(2),
            Signal::TypeFamily { exts } => {
                w.byte(3);
                w.count(exts.len())?;
                for ext in exts {
                    w.text(ext)?;
                }
            }
            Signal::Mapped { ext, created } => {
                w.byte(4);
                w.text(ext)?;
                w.byte(u8::from(*created));
            }
            Signal::Collected { count } => {
                w.byte(5);
                w.count(*count)?;
            }
        }
        total = total.checked_add(group.members.len()).ok_or_else(invalid)?;
        if total > MAX_ROWS {
            return Err(invalid());
        }
        w.count(group.members.len())?;
        for path in &group.members {
            w.path(path)?;
        }
        w.byte(u8::from(group.accepted));
    }
    w.count(plan.untouched.len())?;
    for (path, reason) in &plan.untouched {
        w.path(&TreeSnapshot::path_commitment(path))?;
        match reason {
            Untouched::LooksPersonal(category) => {
                w.byte(0);
                w.byte(match category {
                    Category::Tax => 0,
                    Category::Identity => 1,
                    Category::Medical => 2,
                    Category::Financial => 3,
                    Category::Credential => 4,
                    Category::Legal => 5,
                });
            }
            Untouched::NoClearGroup => w.byte(1),
            Untouched::TooRecent => w.byte(2),
            Untouched::InFlight => w.byte(3),
            Untouched::ProjectDocument => w.byte(4),
            Untouched::NearProjectDocument(marker) => {
                w.byte(5);
                let commitment = TreeSnapshot::path_commitment(Path::new(marker));
                w.text(commitment.to_str().ok_or_else(invalid)?)?;
            }
        }
    }
    for count in [
        plan.scanned,
        plan.skipped_hidden,
        plan.skipped_symlink,
        plan.skipped_system,
        plan.skipped_project,
        plan.skipped_in_flight,
        plan.skipped_package,
        plan.skipped_unreadable,
    ] {
        w.n(count as u64);
    }
    w.byte(u8::from(plan.root_is_synced));
    w.byte(u8::from(plan.allow_sync));
    if snapshot.depth > 8 {
        return Err(invalid());
    }
    w.path(&snapshot.root)?;
    w.n(snapshot.depth as u64);
    w.byte(u8::from(snapshot.whole_units));
    w.identity(&snapshot.root_identity);
    w.entries(&snapshot.ancestors)?;
    w.entries(&snapshot.entries)?;
    if w.0.len() > MAX_BYTES {
        return Err(invalid());
    }
    Ok(w.0)
}

pub(super) fn decode(data: &[u8]) -> io::Result<(Plan, TreeSnapshot, BindingContext)> {
    if data.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut r = Reader { bytes: data, at: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err(invalid());
    }
    let context = BindingContext {
        tool: r.text()?,
        tool_version: r.text()?,
        profile: r.text()?,
        observation_contract: r.text()?,
    };
    let root = r.path()?;
    let group_count = r.count()?;
    let mut groups = Vec::new();
    let mut total = 0usize;
    for _ in 0..group_count {
        let name = r.text()?;
        let signal = match r.byte()? {
            0 => Signal::Screenshot,
            1 => Signal::CameraBurst {
                days: u32::try_from(r.n()?).map_err(|_| invalid())?,
            },
            2 => Signal::Installer,
            3 => {
                let count = r.count()?;
                let mut exts = Vec::new();
                for _ in 0..count {
                    exts.push(r.text()?);
                }
                Signal::TypeFamily { exts }
            }
            4 => Signal::Mapped {
                ext: r.text()?,
                created: r.boolean()?,
            },
            5 => Signal::Collected { count: r.count()? },
            _ => return Err(invalid()),
        };
        let count = r.count()?;
        total = total.checked_add(count).ok_or_else(invalid)?;
        if total > MAX_ROWS {
            return Err(invalid());
        }
        let mut members = Vec::new();
        for _ in 0..count {
            members.push(r.path()?);
        }
        groups.push(Group {
            name,
            signal,
            members,
            accepted: r.boolean()?,
        });
    }
    let count = r.count()?;
    let mut untouched = Vec::new();
    for _ in 0..count {
        let path = r.path()?;
        let reason = match r.byte()? {
            0 => Untouched::LooksPersonal(match r.byte()? {
                0 => Category::Tax,
                1 => Category::Identity,
                2 => Category::Medical,
                3 => Category::Financial,
                4 => Category::Credential,
                5 => Category::Legal,
                _ => return Err(invalid()),
            }),
            1 => Untouched::NoClearGroup,
            2 => Untouched::TooRecent,
            3 => Untouched::InFlight,
            4 => Untouched::ProjectDocument,
            5 => Untouched::NearProjectDocument(r.text()?),
            _ => return Err(invalid()),
        };
        untouched.push((path, reason));
    }
    let mut plan = Plan {
        root,
        groups,
        untouched,
        scanned: r.usize()?,
        skipped_hidden: r.usize()?,
        skipped_symlink: r.usize()?,
        skipped_system: r.usize()?,
        skipped_project: r.usize()?,
        skipped_in_flight: r.usize()?,
        skipped_package: r.usize()?,
        skipped_unreadable: r.usize()?,
        observations: None,
        root_is_synced: r.boolean()?,
        allow_sync: r.boolean()?,
    };
    let root = r.path()?;
    let depth = r.usize()?;
    if depth > 8 {
        return Err(invalid());
    }
    let snapshot = TreeSnapshot {
        root,
        depth,
        whole_units: r.boolean()?,
        root_identity: r.identity()?,
        ancestors: r.entries()?,
        entries: r.entries()?,
    };
    if r.at != data.len() {
        return Err(invalid());
    }
    plan.observations = Some(snapshot.clone());
    Ok((plan, snapshot, context))
}

pub(super) fn digest(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let blocks = bytes.len() / 64 + if bytes.len() % 64 < 56 { 1 } else { 2 };
    let length = (bytes.len() as u64).wrapping_mul(8).to_be_bytes();
    for index in 0..blocks {
        let mut block = [0u8; 64];
        for (offset, slot) in block.iter_mut().enumerate() {
            let position = index * 64 + offset;
            *slot = if position < bytes.len() {
                bytes[position]
            } else if position == bytes.len() {
                0x80
            } else if position >= blocks * 64 - 8 {
                length[position - (blocks * 64 - 8)]
            } else {
                0
            };
        }
        let mut words = [0u32; 64];
        for (word, chunk) in words[..16].iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let x = words[i - 15];
            let y = words[i - 2];
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            words[i] = words[i - 16]
                .wrapping_add(s0)
                .wrapping_add(words[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[i])
                .wrapping_add(words[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (current, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *current = current.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::digest;
    use super::*;

    fn fixture() -> (Plan, TreeSnapshot, BindingContext) {
        let signals = vec![
            Signal::Screenshot,
            Signal::CameraBurst { days: 3 },
            Signal::Installer,
            Signal::TypeFamily {
                exts: vec!["png".into(), "jpg".into()],
            },
            Signal::Mapped {
                ext: "log".into(),
                created: true,
            },
            Signal::Collected { count: 1 },
        ];
        let groups = signals
            .into_iter()
            .enumerate()
            .map(|(number, signal)| Group {
                name: format!("group{number}"),
                signal,
                members: vec![PathBuf::from(format!("/synthetic/root/file{number}"))],
                accepted: number % 2 == 0,
            })
            .collect();
        let reasons = vec![
            Untouched::LooksPersonal(Category::Tax),
            Untouched::LooksPersonal(Category::Identity),
            Untouched::LooksPersonal(Category::Medical),
            Untouched::LooksPersonal(Category::Financial),
            Untouched::LooksPersonal(Category::Credential),
            Untouched::LooksPersonal(Category::Legal),
            Untouched::NoClearGroup,
            Untouched::TooRecent,
            Untouched::InFlight,
            Untouched::ProjectDocument,
            Untouched::NearProjectDocument("project.blend".into()),
        ];
        let plan = Plan {
            root: "/synthetic/root".into(),
            groups,
            untouched: reasons
                .into_iter()
                .enumerate()
                .map(|(number, reason)| {
                    (
                        PathBuf::from(format!("/synthetic/root/held{number}")),
                        reason,
                    )
                })
                .collect(),
            observations: None,
            scanned: 17,
            skipped_hidden: 1,
            skipped_symlink: 2,
            skipped_system: 3,
            skipped_project: 4,
            skipped_in_flight: 5,
            skipped_package: 6,
            skipped_unreadable: 0,
            root_is_synced: false,
            allow_sync: true,
        };
        let identity = SnapshotIdentity {
            device: 1,
            inode: 2,
            size: 3,
            mtime_sec: -4,
            mtime_nsec: 5,
            ctime_sec: 6,
            ctime_nsec: 7,
            mode: 0o40700,
            kind: 2,
        };
        let snapshot = TreeSnapshot {
            root: plan.root.clone(),
            depth: 2,
            whole_units: false,
            root_identity: identity.clone(),
            ancestors: vec![SnapshotEntry {
                path: TreeSnapshot::path_commitment(Path::new("/synthetic/project.blend")),
                identity: identity.clone(),
            }],
            entries: vec![SnapshotEntry {
                path: TreeSnapshot::path_commitment(Path::new("file0")),
                identity,
            }],
        };
        let context = BindingContext {
            tool: "sweep".into(),
            tool_version: "synthetic-version".into(),
            profile: "metadata-only".into(),
            observation_contract: "synthetic-contract".into(),
        };
        (plan, snapshot, context)
    }

    #[test]
    fn every_enum_and_field_round_trips_canonically() {
        let (plan, snapshot, context) = fixture();
        let bytes = encode(&plan, &snapshot, &context).unwrap();
        let (decoded_plan, decoded_snapshot, decoded_context) = decode(&bytes).unwrap();
        assert_eq!(
            encode(&decoded_plan, &decoded_snapshot, &decoded_context).unwrap(),
            bytes
        );
        assert_eq!(decoded_snapshot, snapshot);
        assert_eq!(decoded_plan.groups.len(), 6);
        assert_eq!(decoded_plan.untouched.len(), 11);
    }

    #[test]
    fn held_names_are_commitments_and_reencoding_is_idempotent() {
        let (plan, snapshot, context) = fixture();
        let bytes = encode(&plan, &snapshot, &context).unwrap();
        for name in ["held0", "held10", "project.blend"] {
            assert!(
                !bytes
                    .windows(name.len())
                    .any(|window| window == name.as_bytes())
            );
        }
        let (plan, snapshot, context) = decode(&bytes).unwrap();
        assert_eq!(encode(&plan, &snapshot, &context).unwrap(), bytes);
        let path = Path::new("/synthetic/private-name");
        let committed = TreeSnapshot::path_commitment(path);
        assert_eq!(TreeSnapshot::path_commitment(&committed), committed);
        assert_ne!(
            committed,
            TreeSnapshot::path_commitment(Path::new("/synthetic/private-name2"))
        );
    }

    #[test]
    fn incomplete_trailing_and_unknown_version_frames_refuse() {
        let (plan, snapshot, context) = fixture();
        let bytes = encode(&plan, &snapshot, &context).unwrap();
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut version = bytes;
        version[MAGIC.len() - 1] = 2;
        assert!(decode(&version).is_err());
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
