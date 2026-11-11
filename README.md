# itch-downloader

A command-line tool for downloading and managing your purchased assets from itch.io using the itch.io API.

## Features

- 🗜️ **Auto-extract**: Automatically unzip downloaded files
- 🔍 **Filtering**: Filter assets by author or title
- ⚡ **Parallel downloads**: Download multiple assets concurrently

Note: due to itch.io's pretty heavy rate limiting we only download 3 packages at once by default and use pretty heavy sleeping in-between downloading (should still be faster than doing this manually).

## Installation

### From Releases

Check out the tab https://github.com/BraedonWooding/itch-downloader/releases to find all the release binaries that you can just download immediately.

Just copy that into your path!

### From Source

1. Make sure you have Rust installed. If not, install it from [rustup.rs](https://rustup.rs/)
2. Clone this repository:
   ```bash
   git clone <repository-url>
   cd itch-downloader
   ```
3. Build and install:
   ```bash
   cargo install --path .
   ```

## Usage

### Authentication

You need an itch.io API key to use this tool. You can get one from your [itch.io account settings](https://itch.io/user/settings/api-keys).

You can provide the API key in two ways:
1. **Command line flag**: `--api-key YOUR_API_KEY`
2. **Environment variable**: Set `ITCH_API_KEY=YOUR_API_KEY`

### Commands

#### List Assets (`ls`)

List all your purchased assets:

```bash
# List all assets
itch-downloader ls

# List assets with API key
itch-downloader ls --api-key YOUR_API_KEY

# Filter by author
itch-downloader ls --author "Krishna" --title "Creature"
```

#### Download Assets (`dl`)

Download your purchased assets:

```bash
# Download all assets
itch-downloader dl

# Download to specific directory
itch-downloader dl --output ./my-assets

# Download and automatically unzip
itch-downloader dl --unzip

# Download with custom concurrency (default: 3)
itch-downloader dl --max-concurrent 5

# Organize files by author and item, and extract ZIPs
itch-downloader dl --layout author-title --unzip

# Include alternate formats such as assets.zip AND assets.rar
itch-downloader dl --all-files

# Download specific assets by filter
itch-downloader dl --author "Krishna" --unzip

# Download assets matching title
itch-downloader dl --title "Minifantasy" --output ./minifantasy
```

### Command Options

#### Global Options
- `--api-key, -a`: Your itch.io API key (or set ITCH_API_KEY environment variable)

#### Filtering Options (available for both `ls` and `dl`)
- `--author`: Filter by author username or display name (contains match)
- `--title`: Filter by game title (contains match)

#### Download Options (for `dl` command)
- `--output, -o`: Output directory for downloads (default: current directory)
- `--max-concurrent`: Maximum number of concurrent purchases downloading (default: 3; must be at least 1). Files within a purchase download sequentially.
- `--all-files`: Download every upload, including alternate extensions with the same filename stem
- `--layout`: `flat` (default), `title`, or `author-title`
- `--unzip`: Automatically extract downloaded ZIP files; other archive formats remain as downloaded files
- `--keep-archive-root`: With `--unzip`, preserve a ZIP's single enclosing folder instead of collapsing it

### File Selection

By default, downloads include one file for each unique filename without its extension, compared case-insensitively. For example, `part1.zip`, `part1.rar`, and `part2.zip` download as `part1.zip` and `part2.zip`. ZIP is preferred when available; otherwise the first format returned by itch.io is used. Compound extensions such as `.tar.gz`, `.tar.bz2`, `.tar.xz`, and `.tar.zst` count as one extension.

Use `--all-files` to include every alternative. Names alone determine grouping; the tool does not compare file contents.

## Output Format

### List Command
The `ls` command displays assets in a table format:
```
ID       Author               Title
-------- -------------------- ----------------------------------------
3690241  Krishna Palacio      Minifantasy👤Portrait Generator
3545506  Krishna Palacio      Minifantasy 🐍 Temple Of The Snake...
3626565  Krishna Palacio      Minifantasy🪄Spell Effects II
3507103  Krishna Palacio      Minifantasy 😈 True Villains I
3462664  Krishna Palacio      Minifantasy 📲 UI Overhaul
3379634  Krishna Palacio      Minifantasy 🌀 Warp Lands
3290995  Krishna Palacio      Minifantasy⚔️True Heroes IV
```

### Download Command
The `dl` command shows:
- Progress for fetching your game library
- Individual progress bars for each download
- Extraction progress when using `--unzip`
- Summary of completed downloads

## File Organization

`--layout flat` downloads files directly into the output directory. `--layout title` puts files under the item title; `--layout author-title` adds the author's username above it. Folder names are sanitized for Windows. Conflicting names within a run receive an ID suffix.

With `--layout author-title --unzip`, a purchase with multiple selected files looks like:
```
output-directory/
└── AuthorUsername/
    └── Item Title/
        ├── part1.zip.contents/
        │   └── extracted files...
        ├── part2.zip.contents/
        │   └── extracted files...
        └── guide.pdf
```

When selection leaves just one ZIP, its contents go directly into the item folder, without the extra `filename.zip.contents` directory. Each ZIP's single enclosing folder is also collapsed automatically; `--keep-archive-root` preserves that enclosing folder.

With `--layout flat --unzip`, downloaded files stay in the output directory and extracted contents go under the item title using the same single/multiple-file rules. The author/title folders chosen by the other layouts are always retained.

ZIP files are removed only after successful extraction. Multiple archives have separate extraction directories, including the original extension, so their contents do not overwrite each other. Existing extraction content causes an error rather than an overwrite, and the ZIP is kept. RAR, 7z, and other non-ZIP formats are not extracted.

Failures are reported per file; remaining files continue downloading. The final summary reports completed and failed operations, with a nonzero exit status if any download or extraction failed.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Disclaimer

This tool is not officially affiliated with itch.io. It uses the public itch.io API to access your purchased assets. Please use responsibly and in accordance with itch.io's terms of service.

This tool was written pretty heavily using AI and while I have verified and written parts of it, it could possibly contain bugs and other issues.
