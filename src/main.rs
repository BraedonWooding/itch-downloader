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
async fn unzip_file(zip_path: &PathBuf, extract_to: &PathBuf) -> Result<()> {
    let zip_path = zip_path.clone();
    let extract_to = extract_to.clone();

    // Run the unzip operation in a blocking task since zip crate is synchronous
    tokio::task::spawn_blocking(move || {
        let file = StdFile::open(&zip_path).context("Failed to open zip file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read zip archive")?;

        // First, extract to a temporary directory to check for single-folder structure
        let temp_extract = extract_to.with_extension("temp_extract");
        std::fs::create_dir_all(&temp_extract)
            .context("Failed to create temporary extraction directory")?;

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
        let entries: Vec<_> = std::fs::read_dir(&temp_extract)
            .context("Failed to read temporary extraction directory")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to list directory entries")?;

        let directories: Vec<_> = entries
            .iter()
            .filter(|entry| entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .collect();

        let files: Vec<_> = entries
            .iter()
            .filter(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .collect();

        // If there's exactly one directory and no files at the root, unwrap it
        if directories.len() == 1 && files.is_empty() {
            let single_dir = directories[0].path();
            std::fs::create_dir_all(&extract_to)
                .context("Failed to create final extraction directory")?;

            // Move contents of the single directory to the target directory
            let move_entries: Vec<_> = std::fs::read_dir(&single_dir)
                .context("Failed to read single directory")?
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to list single directory entries")?;

            for entry in move_entries {
                let source = entry.path();
                let dest = extract_to.join(entry.file_name());
                std::fs::rename(&source, &dest)
                    .context("Failed to move file from single directory")?;
            }
        } else {
            // Multiple items at root, move everything as-is
            std::fs::create_dir_all(&extract_to)
                .context("Failed to create final extraction directory")?;

            for entry in entries {
                let source = entry.path();
                let dest = extract_to.join(entry.file_name());
                std::fs::rename(&source, &dest).context("Failed to move extracted content")?;
            }
        }

        // Clean up temporary directory
        std::fs::remove_dir_all(&temp_extract).context("Failed to remove temporary directory")?;

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
        /// Automatically unzip downloaded files
        #[arg(long)]
        unzip: bool,
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

                    progress_bar.finish_with_message(format!("Downloaded {}", filename));
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
    unzip: bool,
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

    // Create download tasks
    let download_tasks: Vec<_> = filtered_keys
        .into_iter()
        .map(|key| {
            let client = client.clone();
            let output_path = output_path.clone();
            let multi_progress = multi_progress.clone();
            let semaphore = semaphore.clone();

            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();

                // Get uploads for this game
                let uploads = match client.get_game_uploads(key.game_id, key.id).await {
                    Ok(uploads) => uploads,
                    Err(e) => {
                        eprintln!("Failed to get uploads for {}: {}", key.game.title, e);
                        return;
                    }
                };

                // Find zip file (prefer zip over other formats)
                let zip_upload = uploads
                    .iter()
                    .find(|upload| upload.filename.to_lowercase().ends_with(".zip"));

                let upload = match zip_upload.or_else(|| uploads.first()) {
                    Some(upload) => upload,
                    None => {
                        eprintln!("No uploads found for {}", key.game.title);
                        return;
                    }
                };

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
                        &upload.filename,
                        &output_path,
                        progress_bar.clone(),
                    )
                    .await;

                match download_result {
                    Ok(()) => {
                        // If unzip is enabled and the file is a zip, extract it
                        if unzip && upload.filename.to_lowercase().ends_with(".zip") {
                            progress_bar.set_message(format!("Extracting {}", upload.filename));
                            let zip_path = output_path.join(&upload.filename);

                            // Create a directory named after the game for extraction
                            let extract_dir = output_path
                                .join(&key.game.title.replace("/", "_").replace("\\", "_"));

                            match unzip_file(&zip_path, &extract_dir).await {
                                Ok(()) => {
                                    progress_bar.finish_with_message(format!(
                                        "Downloaded and extracted {}",
                                        upload.filename
                                    ));
                                    // Optionally remove the zip file after extraction
                                    let _ = tokio::fs::remove_file(&zip_path).await;
                                }
                                Err(e) => {
                                    progress_bar.finish_with_message(format!(
                                        "Downloaded {} but failed to extract: {}",
                                        upload.filename, e
                                    ));
                                    eprintln!("Failed to extract {}: {}", upload.filename, e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        progress_bar.finish_with_message(format!("Failed: {}", e));
                        eprintln!("Failed to download {}: {}", upload.filename, e);
                    }
                }
            })
        })
        .collect();

    // Wait for all downloads to complete
    for task in download_tasks {
        let _ = task.await;
    }

    println!("All downloads completed!");
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
            unzip,
        } => {
            download_packages(api_key, author, title, output, max_concurrent, unzip).await?;
        }
    }

    Ok(())
}
