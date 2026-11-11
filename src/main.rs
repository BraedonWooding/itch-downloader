#![allow(dead_code)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::stream::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;
use std::fs::File as StdFile;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zip::ZipArchive;

mod downloads;
use downloads::{DownloadOptions, plan_downloads, safe_component, unique_path};

/// Truncate a string to a specific visual width, accounting for Unicode characters
fn truncate_to_width(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }

    if max_width <= 3 {
        return "...".chars().take(max_width).collect();
    }

    let mut result = String::new();
    let mut current_width = 0;

    for ch in s.chars() {
        let char_width = ch.width().unwrap_or(0);
        if current_width + char_width + 3 > max_width {
            // +3 for "..."
            result.push_str("...");
            break;
        }
        result.push(ch);
        current_width += char_width;
    }

    result
}

/// Pad a string to a specific visual width with spaces, accounting for Unicode characters
fn pad_to_width(s: &str, target_width: usize) -> String {
    let current_width = s.width();
    if current_width >= target_width {
        return s.to_string();
    }

    let padding_needed = target_width - current_width;
    format!("{}{}", s, " ".repeat(padding_needed))
}

/// Unzip a file to the specified directory
async fn unzip_file(
    zip_path: &PathBuf,
    extract_to: &PathBuf,
    keep_archive_root: bool,
) -> Result<()> {
    let zip_path = zip_path.clone();
    let extract_to = extract_to.clone();

    // Run the unzip operation in a blocking task since zip crate is synchronous
    tokio::task::spawn_blocking(move || {
        let file = StdFile::open(&zip_path).context("Failed to open zip file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read zip archive")?;

        // First, extract to a temporary directory to check for single-folder structure
        std::fs::create_dir_all(extract_to.parent().context("Missing extraction parent")?)?;
        let staging = tempfile::tempdir_in(extract_to.parent().unwrap())
            .context("Failed to create temporary extraction directory")?;
        let temp_extract = staging.path();

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .context("Failed to get file from archive")?;
            let outpath = match file.enclosed_name() {
                Some(path) => temp_extract.join(path),
                None => continue,
            };

            if file.is_dir() {
                std::fs::create_dir_all(&outpath).context("Failed to create directory")?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p).context("Failed to create parent directory")?;
                    }
                }
                let mut outfile =
                    StdFile::create(&outpath).context("Failed to create output file")?;
                std::io::copy(&mut file, &mut outfile).context("Failed to extract file")?;
            }
        }

        // Check if the temporary directory contains only one subdirectory
        let entries: Vec<_> = std::fs::read_dir(temp_extract)
            .context("Failed to read temporary extraction directory")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to list directory entries")?;

        // If there's exactly one directory and no files at the root, unwrap it
        let entries =
            if !keep_archive_root && entries.len() == 1 && entries[0].file_type()?.is_dir() {
                std::fs::read_dir(entries[0].path())
                    .context("Failed to read single directory")?
                    .collect::<Result<Vec<_>, _>>()
                    .context("Failed to list single directory entries")?
            } else {
                entries
            };
        // Check before moving anything; preserve the ZIP if extraction would overwrite content.
        for entry in &entries {
            let destination = extract_to.join(entry.file_name());
            anyhow::ensure!(
                !destination.try_exists()?,
                "Extraction destination already exists: {}",
                destination.display()
            );
        }
        std::fs::create_dir_all(&extract_to)
            .context("Failed to create final extraction directory")?;
        for entry in entries {
            std::fs::rename(entry.path(), extract_to.join(entry.file_name()))
                .context("Failed to move extracted content")?;
        }

        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("Unzip task failed")??;

    Ok(())
}

/// Make an HTTP request with retry logic for 429 errors
async fn make_request_with_retry(
    client: &Client,
    url: &str,
    query_params: &[(&str, u64)],
    api_key: &str,
    max_retries: u32,
) -> Result<reqwest::Response> {
    let mut attempt = 0;

    loop {
        let response = client
            .get(url)
            .bearer_auth(api_key)
            .query(query_params)
            .send()
            .await
            .context("Failed to send request to itch.io API")?;

        match response.status() {
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                attempt += 1;
                if attempt > max_retries {
                    return Err(anyhow::anyhow!(
                        "Too many requests (429) - exceeded max retries ({})",
                        max_retries
                    ));
                }

                let retry_delay = Duration::from_millis(1000 + (attempt as u64 * 500)); // 1s, 1.5s, 2s, etc.
                println!(
                    "Rate limited (429), retrying in {:?} (attempt {}/{})",
                    retry_delay, attempt, max_retries
                );
                sleep(retry_delay).await;
                continue;
            }
            _ => return Ok(response),
        }
    }
}

#[derive(Parser)]
#[command(name = "itch-downloader")]
#[command(about = "A CLI tool for interacting with itch.io API")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all your packages available on itch.io
    Ls {
        /// Your itch.io API key (can also be set via ITCH_API_KEY environment variable)
        #[arg(short, long)]
        api_key: Option<String>,
        /// Filter by author username or display name
        #[arg(long)]
        author: Option<String>,
        /// Filter by title (contains match)
        #[arg(long)]
        title: Option<String>,
    },
    /// Download all matched packages
    Dl {
        /// Your itch.io API key (can also be set via ITCH_API_KEY environment variable)
        #[arg(short, long)]
        api_key: Option<String>,
        /// Filter by author username or display name
        #[arg(long)]
        author: Option<String>,
        /// Filter by title (contains match)
        #[arg(long)]
        title: Option<String>,
        /// Output directory for downloads
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Maximum number of concurrent downloads
        #[arg(long, default_value = "3")]
        max_concurrent: usize,
        #[command(flatten)]
        options: DownloadOptions,
    },
}

#[derive(Debug, Deserialize)]
struct User {
    id: u64,
    username: String,
    display_name: Option<String>,
    url: String,
    cover_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Game {
    id: u64,
    title: String,
    short_text: Option<String>,
    url: String,
    #[serde(rename = "type")]
    game_type: String,
    classification: String,
    created_at: String,
    published_at: Option<String>,
    cover_url: Option<String>,
    still_cover_url: Option<String>,
    min_price: Option<u64>,
    user: User,
}

#[derive(Debug, Deserialize)]
struct OwnedKey {
    id: u64,
    game_id: u64,
    purchase_id: Option<u64>,
    downloads: u64,
    created_at: String,
    updated_at: String,
    game: Game,
}

#[derive(Debug, Deserialize)]
struct Upload {
    id: u64,
    filename: String,
    size: u64,
    #[serde(rename = "type")]
    upload_type: String,
    game_id: u64,
}

#[derive(Debug, Deserialize)]
struct UploadsResponse {
    uploads: Vec<Upload>,
}

#[derive(Debug, Deserialize)]
struct OwnedKeysResponse {
    owned_keys: Vec<OwnedKey>,
    page: u64,
    per_page: u64,
}

#[derive(Clone)]
struct ItchClient {
    client: Client,
    api_key: String,
}

impl ItchClient {
    fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    async fn list_owned_keys(&self) -> Result<Vec<OwnedKey>> {
        let url = "https://api.itch.io/profile/owned-keys";
        let mut all_owned_keys = Vec::new();
        let mut page = 1;

        loop {
            println!("Fetching page {}...", page);

            let response = make_request_with_retry(
                &self.client,
                url,
                &[("page", page)],
                &self.api_key,
                3, // max retries
            )
            .await?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "API request failed with status {}: {}",
                    status,
                    text
                ));
            }

            let owned_keys_response: OwnedKeysResponse = response
                .json()
                .await
                .context("Failed to parse JSON response")?;

            let keys_count = owned_keys_response.owned_keys.len();
            all_owned_keys.extend(owned_keys_response.owned_keys);

            // If we got fewer keys than the per_page limit, we've reached the end
            if keys_count < owned_keys_response.per_page as usize {
                break;
            }

            page += 1;
        }

        println!(
            "Fetched {} total packages across {} pages.",
            all_owned_keys.len(),
            page
        );
        Ok(all_owned_keys)
    }

    async fn get_game_uploads(&self, game_id: u64, download_key_id: u64) -> Result<Vec<Upload>> {
        let url = format!("https://api.itch.io/games/{}/uploads", game_id);

        // Add delay before making request to avoid rate limiting
        sleep(Duration::from_millis(1000)).await;

        let response = make_request_with_retry(
            &self.client,
            &url,
            &[("download_key_id", download_key_id)],
            &self.api_key,
            3, // max retries
        )
        .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "API request failed with status {}: {}",
                status,
                text
            ));
        }

        let uploads_response: UploadsResponse = response
            .json()
            .await
            .context("Failed to parse JSON response")?;

        Ok(uploads_response.uploads)
    }

    async fn download_file(
        &self,
        upload_id: u64,
        download_key_id: u64,
        filename: &str,
        output_path: &PathBuf,
        progress_bar: ProgressBar,
    ) -> Result<()> {
        let url = format!(
            "https://api.itch.io/uploads/{}/download?download_key_id={}",
            upload_id, download_key_id
        );

        // Add delay before making request to avoid rate limiting
        sleep(Duration::from_millis(1000)).await;

        let mut attempt = 0;
        let max_retries = 3;

        loop {
            let response = self
                .client
                .get(&url)
                .bearer_auth(&self.api_key)
                .send()
                .await
                .context("Failed to send download request")?;

            match response.status() {
                reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    attempt += 1;
                    if attempt > max_retries {
                        return Err(anyhow::anyhow!(
                            "Download failed: Too many requests (429) - exceeded max retries ({})",
                            max_retries
                        ));
                    }

                    let retry_delay = Duration::from_millis(1000 + (attempt as u64 * 500));
                    progress_bar.set_message(format!(
                        "Rate limited, retrying {} in {:?}...",
                        filename, retry_delay
                    ));
                    sleep(retry_delay).await;
                    progress_bar.set_message(format!("Downloading {}", filename));
                    continue;
                }
                status if !status.is_success() => {
                    let text = response.text().await.unwrap_or_default();
                    return Err(anyhow::anyhow!(
                        "Download request failed with status {}: {}",
                        status,
                        text
                    ));
                }
                _ => {
                    // Success, proceed with download
                    let total_size = response.content_length().unwrap_or(0);
                    progress_bar.set_length(total_size);

                    let file_path = output_path.join(filename);
                    let mut file = File::create(&file_path)
                        .await
                        .context("Failed to create output file")?;

                    let mut stream = response.bytes_stream();
                    let mut downloaded = 0u64;

                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.context("Failed to read chunk from response")?;
                        file.write_all(&chunk)
                            .await
                            .context("Failed to write chunk to file")?;
                        downloaded += chunk.len() as u64;
                        progress_bar.set_position(downloaded);
                    }

                    file.flush()
                        .await
                        .context("Failed to flush downloaded file")?;
                    return Ok(());
                }
            }
        }
    }
}

async fn list_packages(
    api_key: Option<String>,
    author_filter: Option<String>,
    title_filter: Option<String>,
) -> Result<()> {
    let api_key = api_key
        .or_else(|| std::env::var("ITCH_API_KEY").ok())
        .context("API key is required. Provide it via --api-key flag or ITCH_API_KEY environment variable")?;

    let client = ItchClient::new(api_key);
    let owned_keys = client.list_owned_keys().await?;

    let mut filtered_keys = owned_keys;

    // Apply author filter
    if let Some(author) = &author_filter {
        filtered_keys.retain(|key| {
            key.game
                .user
                .username
                .to_lowercase()
                .contains(&author.to_lowercase())
                || key
                    .game
                    .user
                    .display_name
                    .as_ref()
                    .map(|name| name.to_lowercase().contains(&author.to_lowercase()))
                    .is_some_and(|b| b)
        });
    }

    // Apply title filter
    if let Some(title) = &title_filter {
        filtered_keys.retain(|key| {
            key.game
                .title
                .to_lowercase()
                .contains(&title.to_lowercase())
        });
    }

    if filtered_keys.is_empty() {
        println!("No packages found.");
        return Ok(());
    }

    println!("Your itch.io packages:");
    println!("{:<8} {:<20} {:<40}", "ID", "Author", "Title");
    println!("{:-<8} {:-<20} {:-<40}", "", "", "");

    for key in filtered_keys {
        let title = truncate_to_width(&key.game.title, 37);
        let title_padded = pad_to_width(&title, 40);

        let author_name = key.game.user.display_name.unwrap_or(key.game.user.username);
        let author = truncate_to_width(&author_name, 17);
        let author_padded = pad_to_width(&author, 20);

        println!("{:<8} {} {}", key.game.id, author_padded, title_padded);
    }

    Ok(())
}

async fn download_packages(
    api_key: Option<String>,
    author_filter: Option<String>,
    title_filter: Option<String>,
    output_path: PathBuf,
    max_concurrent: usize,
    options: DownloadOptions,
) -> Result<()> {
    anyhow::ensure!(max_concurrent > 0, "--max-concurrent must be at least 1");
    let api_key = api_key
        .or_else(|| std::env::var("ITCH_API_KEY").ok())
        .context("API key is required. Provide it via --api-key flag or ITCH_API_KEY environment variable")?;

    let client = ItchClient::new(api_key);
    let owned_keys = client.list_owned_keys().await?;

    let mut filtered_keys = owned_keys;

    // Apply author filter
    if let Some(author) = &author_filter {
        filtered_keys.retain(|key| {
            key.game
                .user
                .username
                .to_lowercase()
                .contains(&author.to_lowercase())
                || key
                    .game
                    .user
                    .display_name
                    .as_ref()
                    .map(|name| name.to_lowercase().contains(&author.to_lowercase()))
                    .is_some_and(|b| b)
        });
    }

    // Apply title filter
    if let Some(title) = &title_filter {
        filtered_keys.retain(|key| {
            key.game
                .title
                .to_lowercase()
                .contains(&title.to_lowercase())
        });
    }

    if filtered_keys.is_empty() {
        println!("No packages found to download.");
        return Ok(());
    }

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(&output_path)
        .await
        .context("Failed to create output directory")?;

    println!("Found {} packages to download", filtered_keys.len());

    let multi_progress = MultiProgress::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut item_paths = std::collections::HashSet::new();
    let packages: Vec<_> = filtered_keys
        .into_iter()
        .map(|key| {
            let author_path = output_path.join(safe_component(&key.game.user.username));
            let parent = if options.layout == downloads::Layout::AuthorTitle {
                author_path
            } else {
                output_path.clone()
            };
            let item_path = unique_path(
                &parent.join(safe_component(&key.game.title)),
                key.game_id,
                &mut item_paths,
            );
            (key, item_path)
        })
        .collect();
    let reserved_paths = std::sync::Arc::new(std::sync::Mutex::new(item_paths));

    // Create download tasks; each purchase processes its files sequentially.
    let download_tasks: Vec<_> = packages
        .into_iter()
        .map(|(key, item_path)| {
            let client = client.clone();
            let output_path = output_path.clone();
            let multi_progress = multi_progress.clone();
            let semaphore = semaphore.clone();
            let reserved_paths = reserved_paths.clone();
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();

                // Get uploads for this game
                let uploads = match client.get_game_uploads(key.game_id, key.id).await {
                    Ok(uploads) => uploads,
                    Err(e) => {
                        eprintln!("Failed to get uploads for {}: {}", key.game.title, e);
                        return (0usize, 1usize);
                    }
                };

                let plans = plan_downloads(
                    uploads,
                    &output_path,
                    &item_path,
                    options,
                    &mut reserved_paths.lock().unwrap(),
                );
                if plans.is_empty() {
                    eprintln!("No uploads found for {}", key.game.title);
                    return (0, 0);
                }
                let mut completed = 0;
                let mut failed = 0;
                for plan in plans {
                    let upload = &plan.upload;
                    let filename = plan.path.file_name().unwrap().to_string_lossy();
                    let output_dir = plan.path.parent().unwrap().to_path_buf();
                    if let Err(e) = tokio::fs::create_dir_all(&output_dir).await {
                        eprintln!("Failed to create directory for {}: {}", upload.filename, e);
                        failed += 1;
                        continue;
                    }

                    // Create progress bar
                    let progress_bar = multi_progress.add(ProgressBar::new(upload.size));
                    progress_bar.set_style(
                        ProgressStyle::default_bar()
                            .template("{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                            .unwrap()
                            .progress_chars("#>-"),
                    );
                    progress_bar.set_message(format!("Downloading {}", upload.filename));

                    // Download the file
                    let download_result = client
                        .download_file(
                            upload.id,
                            key.id,
                            &filename,
                            &output_dir,
                            progress_bar.clone(),
                        )
                        .await;

                    match download_result {
                        Ok(()) => {
                            // If unzip is enabled and the file is a zip, extract it
                            if let Some(extract_dir) = plan.extract_to {
                                progress_bar.set_message(format!("Extracting {}", upload.filename));
                                let zip_path = &plan.path;
                                match unzip_file(zip_path, &extract_dir, options.keep_archive_root)
                                    .await
                                {
                                    Ok(()) => {
                                        progress_bar.finish_with_message(format!(
                                            "Downloaded and extracted {}",
                                            upload.filename
                                        ));
                                        // Optionally remove the zip file after extraction
                                        if let Err(e) = tokio::fs::remove_file(zip_path).await {
                                            eprintln!(
                                                "Failed to remove {} after extraction: {}",
                                                filename, e
                                            );
                                        }
                                        completed += 1;
                                    }
                                    Err(e) => {
                                        progress_bar.finish_with_message(format!(
                                            "Downloaded {} but failed to extract: {}",
                                            upload.filename, e
                                        ));
                                        eprintln!("Failed to extract {}: {}", upload.filename, e);
                                        failed += 1;
                                    }
                                }
                            } else {
                                progress_bar
                                    .finish_with_message(format!("Downloaded {}", upload.filename));
                                completed += 1;
                            }
                        }
                        Err(e) => {
                            progress_bar.finish_with_message(format!("Failed: {}", e));
                            eprintln!("Failed to download {}: {}", upload.filename, e);
                            failed += 1;
                        }
                    }
                }
                (completed, failed)
            })
        })
        .collect();

    // Wait for all downloads to complete
    let mut completed = 0;
    let mut failed = 0;
    for task in download_tasks {
        match task.await {
            Ok((successes, failures)) => {
                completed += successes;
                failed += failures;
            }
            Err(e) => {
                eprintln!("Download task failed: {}", e);
                failed += 1;
            }
        }
    }

    println!(
        "Downloads finished: {} completed, {} failed.",
        completed, failed
    );
    anyhow::ensure!(failed == 0, "Some downloads or extractions failed");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ls {
            api_key,
            author,
            title,
        } => {
            list_packages(api_key, author, title).await?;
        }
        Commands::Dl {
            api_key,
            author,
            title,
            output,
            max_concurrent,
            options,
        } => {
            download_packages(api_key, author, title, output, max_concurrent, options).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip(path: &std::path::Path, entries: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(StdFile::create(path).unwrap());
        for (name, contents) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn parses_download_options_and_requires_unzip_for_root_flag() {
        let cli = Cli::try_parse_from([
            "itch-downloader",
            "dl",
            "--all-files",
            "--layout",
            "author-title",
            "--unzip",
            "--keep-archive-root",
        ])
        .unwrap();
        let Commands::Dl { options, .. } = cli.command else {
            panic!("Expected dl")
        };
        assert!(options.all_files && options.unzip && options.keep_archive_root);
        assert_eq!(options.layout, downloads::Layout::AuthorTitle);
        assert!(Cli::try_parse_from(["itch-downloader", "dl", "--keep-archive-root"]).is_err());
    }

    #[tokio::test]
    async fn extracts_into_author_title_with_optional_archive_root() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("assets.zip");
        write_zip(&archive, &[("original/sprite.txt", "sprite")]);
        let item = temp.path().join("author/title");
        unzip_file(&archive, &item, false).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(item.join("sprite.txt")).unwrap(),
            "sprite"
        );
        assert!(!item.join("original").exists());
        let preserved = temp.path().join("author/preserved");
        unzip_file(&archive, &preserved, true).await.unwrap();
        assert!(preserved.join("original/sprite.txt").exists());
    }

    #[tokio::test]
    async fn multiple_zips_with_identical_contents_are_isolated() {
        let temp = tempfile::tempdir().unwrap();
        for (name, content) in [("part1.zip", "one"), ("part2.zip", "two")] {
            let archive = temp.path().join(name);
            write_zip(&archive, &[("root/shared.txt", content)]);
            unzip_file(
                &archive,
                &temp
                    .path()
                    .join("author/title")
                    .join(format!("{name}.contents")),
                false,
            )
            .await
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("author/title/part1.zip.contents/shared.txt")
            )
            .unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("author/title/part2.zip.contents/shared.txt")
            )
            .unwrap(),
            "two"
        );
    }

    #[tokio::test]
    async fn extraction_conflict_preserves_existing_files_and_archive() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("assets.zip");
        write_zip(&archive, &[("shared.txt", "new"), ("other.txt", "other")]);
        let item = temp.path().join("item");
        std::fs::create_dir_all(&item).unwrap();
        std::fs::write(item.join("shared.txt"), "existing").unwrap();
        assert!(unzip_file(&archive, &item, false).await.is_err());
        assert_eq!(
            std::fs::read_to_string(item.join("shared.txt")).unwrap(),
            "existing"
        );
        assert!(!item.join("other.txt").exists());
        assert!(archive.exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
