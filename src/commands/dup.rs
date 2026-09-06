use crate::{
    project::{MetadataExt as _, PackageExt as _, CROSS_SUFFIX},
    shell::ColorChoice,
};
use anyhow::bail;
use std::path::PathBuf;
use structopt::StructOpt;
use strum::VariantNames as _;

#[derive(StructOpt, Debug)]
pub struct OptCompeteDup {
    /// Overwrite the destination if it already exists
    #[structopt(long)]
    pub force: bool,

    /// Existing package
    #[structopt(short, long, value_name("SPEC"))]
    pub package: Option<String>,

    /// Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(ColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: ColorChoice,

    /// Name or alias for a `bin`/`example`
    pub name_or_alias: String,
}

/// Copy a solution to its cross-check companion, `src/bin/<alias>_cross.rs`.
///
/// The copy is a starting point for a brute force, so it duplicates the *current*
/// solution rather than the template: the `input!` block is already correct and
/// only the solving part has to be replaced. Cargo discovers the new file as a
/// target on its own, so `Cargo.toml` is left untouched.
pub(crate) fn run(opt: OptCompeteDup, ctx: crate::Context<'_>) -> anyhow::Result<()> {
    let OptCompeteDup {
        force,
        package,
        manifest_path,
        color,
        name_or_alias,
    } = opt;

    let crate::Context {
        cwd,
        cookies_path: _,
        shell,
    } = ctx;

    shell.set_color_choice(color);

    let manifest_path = manifest_path
        .map(|p| Ok(cwd.join(p.strip_prefix(".").unwrap_or(&p))))
        .unwrap_or_else(|| crate::project::locate_project(&cwd))?;
    let metadata = crate::project::cargo_metadata(manifest_path, &cwd)?;
    let member = metadata.query_for_member(package.as_deref())?;
    let package_metadata = member.read_package_metadata(shell)?;

    let (bin_name, pkg_md_bin) = package_metadata.bin_like_by_name_or_alias(&name_or_alias)?;
    if pkg_md_bin.alias.ends_with(CROSS_SUFFIX) || bin_name.ends_with(CROSS_SUFFIX) {
        bail!("`{name_or_alias}` is already a cross-check target");
    }
    let src = &member.bin_like_target_by_name(bin_name)?.src_path;

    let dst = member
        .manifest_dir()
        .join("src")
        .join("bin")
        .join(format!("{}{CROSS_SUFFIX}.rs", pkg_md_bin.alias));

    let existed = dst.exists();
    if existed && !force {
        bail!("`{dst}` already exists. run with `--force` to overwrite it");
    }

    crate::fs::create_dir_all(dst.parent().expect("joined above"))?;
    crate::fs::copy(src, &dst)?;

    shell.status(
        if existed { "Replaced" } else { "Created" },
        format!("`{dst}` from `{src}`"),
    )?;
    shell.status(
        "Next",
        format!(
            "edit it into a brute force, then `cargo compete test {} --cross`",
            pkg_md_bin.alias,
        ),
    )?;
    Ok(())
}
