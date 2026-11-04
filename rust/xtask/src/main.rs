//! Native packaging without a Python build-time dependency.
use clap::Parser;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const BINARIES: &[&str] = &["yanzhizhou", "deeptutor", "deeptutor-server"];
const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

#[derive(Parser)]
#[command(version, about = "Build and package the Lorevia native runtime")]
struct Options {
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    archive: Option<PathBuf>,
    #[arg(long)]
    target: Option<String>,
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long)]
    target_dir: Option<PathBuf>,
    #[arg(long)]
    cargo: Option<PathBuf>,
    #[arg(long)]
    offline: bool,
}
fn main() {
    if let Err(error) = run(Options::parse()) {
        eprintln!("Native build failed: {error}");
        std::process::exit(1);
    }
}
fn command_output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn hash(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn source(root: &Path) -> Result<Value> {
    if root.join(".git").exists() {
        let git = |args: &[&str]| command_output(Command::new("git").args(args).current_dir(root));
        return Ok(
            json!({"commit":git(&["rev-parse","HEAD"])?,"dirty":!git(&["status","--porcelain","--untracked-files=normal"])?.is_empty()}),
        );
    }
    if root.join("BUILD_SOURCE.json").exists() {
        return Ok(serde_json::from_slice(&fs::read(
            root.join("BUILD_SOURCE.json"),
        )?)?);
    }
    Ok(
        json!({"commit":fs::read_to_string(root.join("BUILD_COMMIT")).ok().map(|s|s.trim().to_owned()),"dirty":null}),
    )
}
fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    })
}
fn fresh(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(format!("Output already exists: {}", path.display()).into());
    }
    Ok(())
}
fn archive(bundle: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().ok_or("Invalid archive path")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let gzip = flate2::GzBuilder::new()
            .mtime(0)
            .write(temporary.as_file_mut(), flate2::Compression::default());
        let mut tar = tar::Builder::new(gzip);
        let mut files = fs::read_dir(bundle)?
            .map(|r| r.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        files.sort();
        for file in files {
            let name = file.file_name().ok_or("Invalid bundle filename")?;
            let mut header = tar::Header::new_gnu();
            header.set_size(fs::metadata(&file)?.len());
            header.set_mode(
                if name.to_string_lossy().starts_with("deeptutor")
                    || name.to_string_lossy().starts_with("yanzhizhou")
                {
                    0o755
                } else {
                    0o644
                },
            );
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            tar.append_data(
                &mut header,
                Path::new("native").join(name),
                fs::File::open(file)?,
            )?;
        }
        tar.into_inner()?.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(destination)?;
    let checksum = destination.with_file_name(format!(
        "{}.sha256",
        destination.file_name().unwrap().to_string_lossy()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(checksum)?;
    writeln!(
        file,
        "{}  {}",
        hash(destination)?,
        destination.file_name().unwrap().to_string_lossy()
    )?;
    Ok(())
}
fn run(options: Options) -> Result<()> {
    let root = options.root.canonicalize()?;
    let cargo = options
        .cargo
        .or_else(|| std::env::var_os("CARGO").map(PathBuf::from))
        .unwrap_or_else(|| "cargo".into());
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| cargo.with_file_name(if cfg!(windows) { "rustc.exe" } else { "rustc" }));
    let verbose = command_output(Command::new(&rustc).arg("-vV"))?;
    let host = verbose
        .lines()
        .find_map(|s| s.strip_prefix("host: "))
        .ok_or("Unable to identify Rust host")?;
    let target = options.target.as_deref().unwrap_or(host);
    if !TARGETS.contains(&target) {
        return Err("Unsupported native target".into());
    }
    let destination = absolute(&options.output)?;
    fresh(&destination)?;
    let archive_path = options.archive.as_deref().map(absolute).transpose()?;
    if let Some(path) = &archive_path {
        fresh(path)?;
        fresh(&path.with_file_name(
            format!("{}.sha256",path.file_name().ok_or("Invalid archive name")?.to_string_lossy()),
        ))?;
        if path.starts_with(&destination) {
            return Err("Archive must be outside the native bundle directory".into());
        }
    }
    let target_dir = options
        .target_dir
        .as_deref()
        .map(absolute)
        .transpose()?
        .unwrap_or_else(|| root.join("target"));
    let mut build = Command::new(&cargo);
    build
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "deeptutor-server",
            "--target",
            target,
            "--target-dir",
        ])
        .arg(&target_dir);
    if options.offline {
        build.arg("--offline");
    }
    if !build.status()?.success() {
        return Err("Cargo build failed".into());
    }
    let parent = destination.parent().ok_or("Invalid output directory")?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let bundle = staging.path().join("native");
    fs::create_dir(&bundle)?;
    let mut files = serde_json::Map::new();
    for name in BINARIES {
        let name = format!(
            "{name}{}",
            if target.contains("windows") {
                ".exe"
            } else {
                ""
            }
        );
        let source = target_dir.join(target).join("release").join(&name);
        if !fs::symlink_metadata(&source)?.file_type().is_file() {
            return Err("Native binary must be a regular file".into());
        }
        let installed = bundle.join(&name);
        fs::copy(source, &installed)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&installed, fs::Permissions::from_mode(0o755))?;
        }
        files.insert(
            name,
            json!({"sha256":hash(&installed)?,"size":fs::metadata(installed)?.len()}),
        );
    }
    for license in ["LICENSE", "THIRD_PARTY_NOTICES.txt"] {
        fs::copy(root.join(license), bundle.join(license))?;
    }
    let manifest = json!({"schema_version":1,"target":target,"compiler":verbose.lines().next(),"source":source(&root)?,"cargo_lock_sha256":hash(&root.join("Cargo.lock"))?,"files":files});
    let mut manifest_file = fs::File::create(bundle.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut manifest_file, &manifest)?;
    writeln!(manifest_file)?;
    // Refuse concurrent overwrite as well as pre-existing output.
    fs::create_dir(&destination)?;
    for file in fs::read_dir(&bundle)? {
        let file = file?;
        fs::rename(file.path(), destination.join(file.file_name()))?;
    }
    if let Some(path) = archive_path {
        archive(&destination, &path)?;
    }
    println!("Native runtime bundle: {}", destination.display());
    Ok(())
}
