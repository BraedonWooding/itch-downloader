# itch-downloader

A command-line tool for downloading and managing your purchased games from itch.io using the itch.io API.

## Features

- 📋 **List your games**: View all your purchased games from itch.io
- ⬇️ **Download games**: Download your purchased games with progress tracking
- 🗜️ **Auto-extract**: Automatically unzip downloaded files
- 🔍 **Filtering**: Filter games by author or title
- ⚡ **Parallel downloads**: Download multiple games concurrently
- 🌍 **Unicode support**: Proper handling of emojis and Unicode characters in game titles
- 📊 **Progress tracking**: Real-time download progress with visual progress bars

## Installation

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

#### List Games (`ls`)

List all your purchased games:

```bash
# List all games
itch-downloader ls

# List games with API key
itch-downloader ls --api-key YOUR_API_KEY

# Filter by author
itch-downloader ls --author "Supergiant Games"

# Filter by title (contains match)
itch-downloader ls --title "puzzle"

# Combine filters
itch-downloader ls --author "indie" --title "adventure"
```

#### Download Games (`dl`)

Download your purchased games:

```bash
# Download all games
itch-downloader dl

# Download to specific directory
itch-downloader dl --output ./my-games

# Download and automatically unzip
itch-downloader dl --unzip

# Download with custom concurrency (default: 16)
itch-downloader dl --max-concurrent 5

# Download specific games by filter
itch-downloader dl --author "Klei Entertainment" --unzip

# Download games matching title
itch-downloader dl --title "soundtrack" --output ./music
```

### Command Options

#### Global Options
- `--api-key, -a`: Your itch.io API key (or set ITCH_API_KEY environment variable)

#### Filtering Options (available for both `ls` and `dl`)
- `--author`: Filter by author username or display name (contains match)
- `--title`: Filter by game title (contains match)

#### Download Options (for `dl` command)
- `--output, -o`: Output directory for downloads (default: current directory)
- `--max-concurrent`: Maximum number of concurrent downloads (default: 16)
- `--unzip`: Automatically extract downloaded ZIP files

## Examples

### Basic Usage

```bash
# Set API key as environment variable
export ITCH_API_KEY="your_api_key_here"

# List all your games
itch-downloader ls

# Download all games to a games folder and extract them
itch-downloader dl --output ./games --unzip
```

### Advanced Filtering

```bash
# Find all puzzle games
itch-downloader ls --title "puzzle"

# Download all games from a specific author
itch-downloader dl --author "Team Cherry" --unzip

# Download soundtracks only
itch-downloader dl --title "soundtrack" --output ./music
```

### Bulk Operations

```bash
# Download everything with controlled concurrency
itch-downloader dl --max-concurrent 3 --unzip --output ./complete-library

# Download only games with "demo" in the title
itch-downloader dl --title "demo" --output ./demos
```

## Output Format

### List Command
The `ls` command displays games in a table format:
```
ID       Author               Title
-------- -------------------- ----------------------------------------
1234567  Supergiant Games     Hades
2345678  Team Cherry          Hollow Knight
```

### Download Command
The `dl` command shows:
- Progress for fetching your game library
- Individual progress bars for each download
- Extraction progress when using `--unzip`
- Summary of completed downloads

## File Organization

When using the `--unzip` option, games are organized as follows:
```
output-directory/
├── Game Title 1/
│   ├── extracted files...
├── Game Title 2/
│   ├── extracted files...
└── ...
```

ZIP files are automatically removed after successful extraction.

## Error Handling

The tool handles various error conditions gracefully:
- Invalid API keys
- Network connectivity issues
- Missing or corrupted downloads
- Failed extractions (original ZIP file is preserved)

## Technical Details

- **Language**: Rust
- **Async Runtime**: Tokio
- **HTTP Client**: reqwest
- **Progress Bars**: indicatif
- **CLI Framework**: clap
- **Archive Handling**: zip crate

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Disclaimer

This tool is not officially affiliated with itch.io. It uses the public itch.io API to access your purchased games. Please use responsibly and in accordance with itch.io's terms of service.
