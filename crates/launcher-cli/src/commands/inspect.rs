//! Low-level views of CASC storage and build files.

use crate::*;

pub(crate) fn casc_verify(src: PathBuf, dst: PathBuf, sample: usize) -> Result<()> {
    use lightforge_core::tact::entry::Header;
    use lightforge_core::tact::storage::Storage;

    let mut source = Storage::open(&src).context("opening the source store")?;
    let mut rebuilt = Storage::open(&dst).context("opening the rebuilt store")?;
    let (a, b) = (source.stats(), rebuilt.stats());
    println!("source   {:>9} files, {:>9} entries, {:.1} GB", a.files, a.entries, a.bytes as f64 / 1e9);
    println!("rebuilt  {:>9} files, {:>9} entries, {:.1} GB", b.files, b.entries, b.bytes as f64 / 1e9);
    if a.files != b.files {
        println!("NOTE: file counts differ by {}", a.files.abs_diff(b.files));
    }

    let entries: Vec<_> = rebuilt.files().copied().collect();
    let stride = (entries.len() / sample.max(1)).max(1);
    let chosen: Vec<_> = entries.iter().step_by(stride).take(sample).copied().collect();
    println!("\ncomparing {} entries…", chosen.len());

    let t = std::time::Instant::now();
    let (mut same, mut differ, mut both_enc, mut hdr_ok, mut hdr_bad) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for e in &chosen {
        // The header must verify at the position we chose for it.
        match rebuilt.read_raw(e) {
            Ok(raw) if Header::verify(&raw, e.archive_id, e.offset) => hdr_ok += 1,
            _ => hdr_bad += 1,
        }

        let mut ekey = [0u8; 16];
        ekey[..9].copy_from_slice(&e.key);
        match (source.read(&ekey), rebuilt.read(&ekey)) {
            (Ok(x), Ok(y)) if x == y => same += 1,
            // Encrypted content fails to decode in both, which is
            // agreement, not damage.
            (Err(_), Err(_)) => both_enc += 1,
            _ => {
                differ += 1;
                if differ <= 3 {
                    println!("  MISMATCH at archive {} offset {}", e.archive_id, e.offset);
                }
            }
        }
    }

    println!(
        "\ndecoded identically: {same}\nundecodable in both:  {both_enc}\nmismatched:           {differ}"
    );
    println!("headers verifying:    {hdr_ok} ok, {hdr_bad} bad");
    println!("compared in {:.0}s", t.elapsed().as_secs_f64());
    if differ == 0 && hdr_bad == 0 {
        println!("\nthe rebuilt store is byte-equivalent to the source for every entry checked");
    } else {
        anyhow::bail!("the rebuilt store does not match the source");
    }
    Ok(())
}

pub(crate) fn casc_rebuild(src: PathBuf, dst: PathBuf, percent: u8) -> Result<()> {
    use lightforge_core::tact::storage::Storage;
    use lightforge_core::tact::writer::StoreWriter;

    if dst.starts_with(&src) {
        anyhow::bail!("dst must not be inside src — the source is read-only");
    }
    let dst_data = dst.join("Data").join("data");
    if dst_data.exists() {
        anyhow::bail!("{} already exists; remove it first", dst_data.display());
    }

    // Everything outside the CASC store is small and copied verbatim:
    // binaries, WTF, Data/config, and .build.info.
    println!("copying non-CASC files…");
    let t = std::time::Instant::now();
    let status = std::process::Command::new("rsync")
        .arg("-a")
        .arg("--exclude=Data/data/")
        .arg(format!("{}/", src.display()))
        .arg(format!("{}/", dst.display()))
        .status()
        .context("running rsync — is it installed?")?;
    anyhow::ensure!(status.success(), "rsync failed");
    std::fs::create_dir_all(&dst_data)?;
    println!("  done in {:?}", t.elapsed());

    let mut source = Storage::open(&src).context("opening the source store")?;
    let mut entries: Vec<_> = source.files().copied().collect();
    // Read in physical order: the index is sorted by key, which would
    // make this a random-access pass over 123 GB.
    entries.sort_by_key(|e| (e.archive_id, e.offset));
    if percent < 100 {
        let keep = entries.len() * usize::from(percent.max(1)) / 100;
        entries.truncate(keep);
    }
    println!("rebuilding {} entries", entries.len());

    // Carry the source's index generation numbers over. A real
    // install has exactly one generation per bucket in the 0x32-0x38
    // range; starting from zero produces a store the client does not
    // treat the same way.
    let mut writer = StoreWriter::create(&dst_data)?;
    let src_data = source.data_dir().to_path_buf();
    for bucket in 0..lightforge_core::tact::idx::BUCKETS {
        if let Some(seq) = lightforge_core::tact::idx::latest_sequence(&src_data, bucket)? {
            writer = writer.with_index_generation(bucket, seq);
        }
    }

    let t = std::time::Instant::now();
    let (mut done, mut failed) = (0usize, 0usize);
    let mut last_report = std::time::Instant::now();

    for entry in &entries {
        let raw = match source.read_raw(entry) {
            Ok(r) => r,
            Err(e) => {
                failed += 1;
                if failed <= 3 { println!("  read failed: {e}"); }
                continue;
            }
        };
        // Pad the 9-byte index key back to a full key: the writer uses
        // only the first nine bytes, which is what headers store.
        let mut ekey = [0u8; 16];
        ekey[..9].copy_from_slice(&entry.key);
        writer.write(&ekey, &raw[lightforge_core::tact::idx::CASC_ENTRY_HEADER..])?;
        done += 1;

        if last_report.elapsed().as_secs() >= 5 {
            let secs = t.elapsed().as_secs_f64();
            println!(
                "  {done}/{} ({:.0}%)  {:.1} GB  {:.0} MB/s",
                entries.len(),
                done as f64 / entries.len() as f64 * 100.0,
                writer.bytes_written() as f64 / 1e9,
                writer.bytes_written() as f64 / 1e6 / secs.max(0.001),
            );
            last_report = std::time::Instant::now();
        }
    }

    let summary = writer.finish()?;
    let secs = t.elapsed().as_secs_f64();
    println!(
        "\nrebuilt {} files / {:.1} GB into {} archives + {} indices in {:.0}s ({:.0} MB/s)",
        summary.files,
        summary.bytes as f64 / 1e9,
        summary.archives,
        summary.indices.len(),
        secs,
        summary.bytes as f64 / 1e6 / secs.max(0.001),
    );
    if failed > 0 {
        println!("{failed} source entries could not be read");
    }
    println!("\nverify with:  lightforge content {} --locale enUS", dst.display());
    Ok(())
}

pub(crate) fn content(path: PathBuf, product: String, locale: String, fdid: Option<u32>, file: Option<String>) -> Result<()> {
    use lightforge_core::tact::config::BuildConfig;
    use lightforge_core::tact::content::{Content, Locale};
    use lightforge_core::tact::storage::Storage;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let locale: Locale = locale.parse().context("parsing --locale")?;

    // The build key is already recorded in .build.info, which is the
    // file the `buildinfo` writer maintains. This is where the two
    // halves meet.
    let info = BuildInfo::load(path.join(".build.info"))
        .with_context(|| format!("reading {}", path.join(".build.info").display()))?;
    let build_key = info.get(&code, lightforge_core::buildinfo::COL_BUILD_KEY)?;
    println!("build key       {build_key}");

    let build = BuildConfig::load(path.join("Data").join("config"), build_key)
        .context("loading the build config")?;
    println!("build name      {}", build.build_name.as_deref().unwrap_or("(unnamed)"));

    let mut storage = Storage::open(&path).context("opening the CASC store")?;
    let t = std::time::Instant::now();
    let content = Content::open(&mut storage, build, locale)
        .context("loading the encoding and root manifests")?;
    println!("manifests       loaded in {:?}", t.elapsed());
    println!("content keys    {}", content.encoded_count());
    println!("file ids        {}", content.file_count());

    // Root lists every file in the build; local storage holds only what
    // was actually downloaded for this install's locale and options.
    // The gap is expected, and measuring it is the basis of a future
    // verify/repair pass.
    let sample = 20_000usize;
    let stride = (content.file_count() / sample).max(1);
    let (mut present, mut checked) = (0usize, 0usize);
    for id in content.file_ids().step_by(stride).take(sample) {
        checked += 1;
        if content.ekey_for_fdid(id).is_some_and(|k| storage.contains(&k)) {
            present += 1;
        }
    }
    println!(
        "installed       {present}/{checked} sampled ({:.0}% of the build)",
        present as f64 / checked.max(1) as f64 * 100.0
    );

    let target = match (&fdid, &file) {
        (Some(id), _) => Some(*id),
        (None, Some(path)) => Some(
            content
                .fdid_for_path(path)
                .with_context(|| format!("no file named `{path}` in this build"))?,
        ),
        // Nothing asked for: prove the chain works on the first file
        // that is actually present locally. The first file id overall
        // is usually *not* installed, since root covers the whole
        // build, not this install's subset.
        (None, None) => content
            .file_ids()
            .find(|id| content.ekey_for_fdid(*id).is_some_and(|k| storage.contains(&k))),
    };

    if let Some(id) = target {
        let ckey = content.ckey_for_fdid(id).context("file id has no content key")?;
        let ekey = content.ekey_for_fdid(id).context("content key has no encoding key")?;
        let bytes = content.read_fdid(&mut storage, id).context("reading the file")?;
        println!("\nfile id         {id}");
        println!("content key     {}", lightforge_core::text::hex(&ckey));
        println!("encoding key    {}", lightforge_core::text::hex(&ekey));
        println!("expected size   {}", content.size_of(&ckey).unwrap_or(0));
        println!("decoded size    {}", bytes.len());
        let magic: String = bytes.iter().take(4).map(|b| {
            if b.is_ascii_graphic() { *b as char } else { '.' }
        }).collect();
        println!("magic           {magic:?}");
    }
    Ok(())
}

pub(crate) fn casc(path: PathBuf, sample: usize) -> Result<()> {
    use lightforge_core::tact::storage::{Storage, StorageError};

    let t = std::time::Instant::now();
    let mut storage = Storage::open(&path)
        .with_context(|| format!("opening the CASC store at {}", path.display()))?;
    let stats = storage.stats();
    println!("store           {}", storage.data_dir().display());
    println!("buckets         {}", stats.buckets);
    println!("files           {}", stats.files);
    println!("archive markers {}", stats.entries - stats.files);
    println!("archived        {:.1} GB", stats.bytes as f64 / 1e9);
    println!("opened in       {:?}\n", t.elapsed());

    // Stride the sample so a single bad archive can't pass as healthy.
    let entries: Vec<_> = storage.files().copied().collect();
    let stride = (entries.len() / sample.max(1)).max(1);
    let chosen: Vec<_> = entries.iter().step_by(stride).take(sample).collect();

    let (mut ok, mut encrypted, mut failed) = (0usize, 0usize, 0usize);
    let mut bytes = 0u64;
    let t = std::time::Instant::now();
    for entry in &chosen {
        match storage.read_entry(entry) {
            Ok(d) => {
                ok += 1;
                bytes += d.len() as u64;
            }
            // Unreleased content ships encrypted; without the key this
            // is expected on a healthy install, not damage.
            Err(StorageError::Decode { source, .. })
                if source.to_string().to_lowercase().contains("key") =>
            {
                encrypted += 1
            }
            Err(e) => {
                failed += 1;
                if failed <= 3 {
                    println!("  {e}");
                }
            }
        }
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "decoded {ok}/{} ({encrypted} encrypted, {failed} failed) — {:.1} MB in {:.2}s ({:.0} MB/s)",
        chosen.len(),
        bytes as f64 / 1e6,
        secs,
        bytes as f64 / 1e6 / secs.max(0.001),
    );
    Ok(())
}

pub(crate) fn idx(path: PathBuf) -> Result<()> {
    use lightforge_core::tact::idx::{self, Index};

    // Accept either spelling so this is usable against an install root
    // or a bare data directory.
    let dir = if path.join("Data").join("data").is_dir() {
        path.join("Data").join("data")
    } else {
        path.clone()
    };

    let files = idx::discover(&dir).with_context(|| format!("scanning {}", dir.display()))?;
    if files.is_empty() {
        println!("no .idx files under {}", dir.display());
        return Ok(());
    }

    let (mut exact, mut differing) = (0usize, 0usize);
    let mut entries_total = 0usize;
    for file in &files {
        let raw = std::fs::read(file)?;
        let name = file.file_name().unwrap_or_default().to_string_lossy();

        match Index::parse(&raw) {
            Ok(index) => {
                // Both hashes already verified by parse; this proves we
                // can reproduce the file Blizzard wrote, byte for byte.
                let round = index.to_bytes()?;
                let identical = round == raw;
                if identical { exact += 1 } else { differing += 1 }
                entries_total += index.entries().len();
                println!(
                    "{name}  bucket {:02x}  v{}  {:>7} files  {:>4} markers  {:>5} free  {}",
                    index.bucket(),
                    index.version(),
                    index.files().count(),
                    index.entries().len() - index.files().count(),
                    index.remaining_capacity(),
                    if identical { "round-trips exactly" } else { "DIFFERS" },
                );
            }
            Err(e) => {
                differing += 1;
                println!("{name}  FAILED: {e}");
            }
        }
    }
    println!(
        "\n{} files, {entries_total} entries; {exact} round-trip byte-for-byte, {differing} do not",
        files.len()
    );
    Ok(())
}

pub(crate) fn build_info_set(ctx: Ctx, product: String, build_key: String, cdn_key: String, version: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::buildinfo::BuildInfoError;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let path = layout.build_info(&code);

    // Read-modify-write, atomically. The caller is responsible for
    // having run pre-flight first. This is the file Agent watches.
    let info = BuildInfo::update(&path, |info| {
        info.set_build(&code, &build_key, &cdn_key, &version)
    })
    .map_err(|e: BuildInfoError| e)
    .with_context(|| format!("rewriting {}", path.display()))?;

    println!("{}", path.display());
    println!("{}", info.row(&code).expect("the row just written").cells().join(" | "));
    Ok(())
}

pub(crate) fn build_info(ctx: Ctx, product: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let code: ProductCode = product.parse().expect("product codes are infallible");
    let path = layout.build_info(&code);
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let info = BuildInfo::parse(&contents)
        .with_context(|| format!("parsing {}", path.display()))?;

    let row = info
        .row(&code)
        .with_context(|| format!("`{code}` has no row in {}", path.display()))?;
    println!("{}", row.cells().join(" | "));
    Ok(())
}
