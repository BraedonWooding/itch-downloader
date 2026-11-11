use crate::Upload;
use clap::{Args, ValueEnum};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Layout {
    /// Download files directly into the output directory
    #[default]
    Flat,
    /// Group downloads by item title
    Title,
    /// Group downloads by author username, then item title
    AuthorTitle,
}

#[derive(Debug, Clone, Copy, Default, Args)]
pub struct DownloadOptions {
    /// Download every upload, including alternate formats with the same name
    #[arg(long)]
    pub all_files: bool,
    /// Directory layout for downloaded and extracted files
    #[arg(long, value_enum, default_value = "flat")]
    pub layout: Layout,
    /// Automatically extract ZIP files (other archive formats are kept as files)
    #[arg(long)]
    pub unzip: bool,
    /// Preserve a ZIP's single enclosing folder instead of collapsing it
    #[arg(long, requires = "unzip")]
    pub keep_archive_root: bool,
}

/// Use only a filename, even when the server supplies separators from another OS.
fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn stem(name: &str) -> String {
    let name = basename(name).to_lowercase();
    // Compound tar extensions represent a single archive format.
    for suffix in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return stem.to_owned();
        }
    }
    Path::new(&name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn is_zip(name: &str) -> bool {
    name.to_lowercase().ends_with(".zip")
}

/// Keep API order, preferring ZIP over another format in the same stem group.
fn select_uploads(uploads: Vec<Upload>, all_files: bool) -> Vec<Upload> {
    if all_files {
        return uploads;
    }
    let mut groups = HashMap::new();
    let mut selected: Vec<Upload> = Vec::new();
    for upload in uploads {
        let key = stem(&upload.filename);
        if let Some(&index) = groups.get(&key) {
            let existing: &Upload = &selected[index];
            if is_zip(&upload.filename) && !is_zip(&existing.filename) {
                selected[index] = upload;
            }
        } else {
            groups.insert(key, selected.len());
            selected.push(upload);
        }
    }
    selected
}

/// A portable single path component, including Windows reserved-name handling.
pub fn safe_component(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim_end_matches([' ', '.']);
    if name.is_empty() {
        return "_".to_owned();
    }
    let prefix = name.split('.').next().unwrap().to_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&prefix.as_str())
        || ((prefix.starts_with("COM") || prefix.starts_with("LPT"))
            && prefix.len() == 4
            && matches!(prefix.as_bytes()[3], b'1'..=b'9'))
    {
        format!("_{}", name)
    } else {
        name.to_owned()
    }
}

/// Reserve names case-insensitively so simultaneous packages cannot overwrite each other.
pub fn unique_path(path: &Path, id: u64, reserved: &mut HashSet<String>) -> PathBuf {
    let mut candidate = path.to_path_buf();
    let mut attempt = 0;
    while !reserved.insert(candidate.to_string_lossy().to_lowercase()) {
        attempt += 1;
        let suffix = if attempt == 1 {
            format!("-{id}")
        } else {
            format!("-{id}-{attempt}")
        };
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let extension = path
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy()))
            .unwrap_or_default();
        candidate = path.with_file_name(format!("{stem}{suffix}{extension}"));
    }
    candidate
}

pub struct PlannedDownload {
    pub upload: Upload,
    pub path: PathBuf,
    pub extract_to: Option<PathBuf>,
}

pub fn plan_downloads(
    uploads: Vec<Upload>,
    output: &Path,
    item: &Path,
    options: DownloadOptions,
    reserved: &mut HashSet<String>,
) -> Vec<PlannedDownload> {
    let uploads = select_uploads(uploads, options.all_files);
    let multiple = uploads.len() > 1;
    let directory = if options.layout == Layout::Flat {
        output
    } else {
        item
    };
    // Reserve every downloaded file before choosing extraction directories.
    let mut plans: Vec<_> = uploads
        .into_iter()
        .map(|upload| {
            let path = unique_path(
                &directory.join(safe_component(basename(&upload.filename))),
                upload.id,
                reserved,
            );
            PlannedDownload {
                upload,
                path,
                extract_to: None,
            }
        })
        .collect();
    for plan in &mut plans {
        if options.unzip && is_zip(&plan.upload.filename) {
            plan.extract_to = Some(if multiple {
                unique_path(
                    &item.join(format!(
                        "{}.contents",
                        plan.path.file_name().unwrap().to_string_lossy()
                    )),
                    plan.upload.id,
                    reserved,
                )
            } else {
                // A single selected ZIP needs no extra per-archive folder.
                item.to_path_buf()
            });
        }
    }
    plans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uploads(names: &[&str]) -> Vec<Upload> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| Upload {
                id: i as u64 + 1,
                filename: name.to_string(),
                size: 0,
                upload_type: "default".to_owned(),
                game_id: 1,
            })
            .collect()
    }

    #[test]
    fn selects_all_distinct_stems_and_prefers_zip() {
        let selected = select_uploads(
            uploads(&[
                "Part1.rar",
                "Part2.zip",
                "PART1.ZIP",
                "manual.pdf",
                "Part2.7z",
            ]),
            false,
        );
        assert_eq!(
            selected
                .iter()
                .map(|u| u.filename.as_str())
                .collect::<Vec<_>>(),
            ["PART1.ZIP", "Part2.zip", "manual.pdf"]
        );
    }

    #[test]
    fn compound_archives_and_first_non_zip_alternative() {
        let selected = select_uploads(
            uploads(&[
                "assets.tar.gz",
                "assets.rar",
                "assets.zip",
                "music.7z",
                "music.rar",
            ]),
            false,
        );
        assert_eq!(
            selected
                .iter()
                .map(|u| u.filename.as_str())
                .collect::<Vec<_>>(),
            ["assets.zip", "music.7z"]
        );
    }

    #[test]
    fn all_files_preserves_every_alternative() {
        assert_eq!(
            select_uploads(uploads(&["assets.zip", "assets.rar", "assets.7z"]), true).len(),
            3
        );
        assert!(select_uploads(vec![], false).is_empty());
    }

    #[test]
    fn layouts_and_extraction_isolate_multiple_archives() {
        for layout in [Layout::Flat, Layout::Title, Layout::AuthorTitle] {
            let output = Path::new("output");
            let item = if layout == Layout::AuthorTitle {
                output.join("author/item")
            } else {
                output.join("item")
            };
            let plans = plan_downloads(
                uploads(&["part1.zip", "part2.zip", "guide.pdf"]),
                output,
                &item,
                DownloadOptions {
                    layout,
                    unzip: true,
                    ..Default::default()
                },
                &mut HashSet::new(),
            );
            assert_eq!(
                plans[0].path,
                if layout == Layout::Flat {
                    output.join("part1.zip")
                } else {
                    item.join("part1.zip")
                }
            );
            assert_eq!(plans[0].extract_to, Some(item.join("part1.zip.contents")));
            assert_eq!(plans[1].extract_to, Some(item.join("part2.zip.contents")));
            assert_eq!(plans[2].extract_to, None);
        }
    }

    #[test]
    fn collapse_uses_selected_count_and_all_files_keeps_alternatives_separate() {
        let options = DownloadOptions {
            unzip: true,
            layout: Layout::AuthorTitle,
            ..Default::default()
        };
        let item = Path::new("out/author/title");
        let plans = plan_downloads(
            uploads(&["assets.rar", "assets.zip"]),
            Path::new("out"),
            item,
            options,
            &mut HashSet::new(),
        );
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].extract_to.as_deref(), Some(item));
        let plans = plan_downloads(
            uploads(&["assets.rar", "assets.zip"]),
            Path::new("out"),
            item,
            DownloadOptions {
                all_files: true,
                ..options
            },
            &mut HashSet::new(),
        );
        assert_eq!(plans[0].extract_to, None);
        assert_eq!(plans[1].extract_to, Some(item.join("assets.zip.contents")));
    }

    #[test]
    fn sanitization_and_case_insensitive_collisions() {
        assert_eq!(safe_component("CON.zip"), "_CON.zip");
        assert_eq!(safe_component("../bad:title. "), ".._bad_title");
        assert_eq!(safe_component(".."), "_");
        let plans = plan_downloads(
            uploads(&["dir/a.zip", "A.ZIP", "a.zip.contents"]),
            Path::new("out"),
            Path::new("out/item"),
            DownloadOptions {
                all_files: true,
                unzip: true,
                ..Default::default()
            },
            &mut HashSet::new(),
        );
        assert_eq!(plans[0].path, Path::new("out/a.zip"));
        assert_eq!(plans[1].path, Path::new("out/A-2.ZIP"));
        assert_ne!(plans[0].extract_to, plans[1].extract_to);
    }
}
