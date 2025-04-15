git remote set-url origin git@github.com:your-username/new-repo-name.git
# PowerCrust v1.0
(this is a Rust re-implementation of my powercasts module)

A command-line utility that extracts media files from RSS feeds listed in an OPML file.

## Features

- Parses RSS feeds from an OPML subscription file
- Extracts media URLs (MP3, MP4) from podcast feeds
- Creates a list of all media URLs in a text file
- Generates a separate list of the newest episodes from each feed
- Supports multiple output formats for newest episodes (plain text, Markdown, HTML)
- Optional chronological sorting of episodes in the output file
- Configurable time window for "current" episodes

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (1.55 or newer)
- Cargo (included with Rust)

### Building from source

```bash
# Clone the repository
git clone https://github.com/taext/powercrust.git
cd powercrust

# Build with cargo
cargo build --release

# Optional: copy to your PATH
cp target/release/powercrust /usr/local/bin/
```

## Usage

The basic usage is:

```bash
powercrust [OPTIONS] <OPML_FILE>
```

### Arguments

- `<OPML_FILE>`: Path to the OPML file containing RSS feed subscriptions

### Options

- `-c, --check-current <BOOL>`: Only include episodes published within the specified days (default: true)
- `-d, --days <DAYS>`: Number of days to consider as "current" (default: 30)
- `-o, --chronological <BOOL>`: Sort all episodes chronologically (oldest first) in the output file (default: false)
- `-f, --format <FORMAT>`: Format for newest.txt output (txt, md, html) (default: txt)

### Examples

```bash
# Basic usage - parse feeds from subscriptions.opml
powercrust subscriptions.opml

# Include all episodes, not just recent ones
powercrust --check-current false subscriptions.opml

# Consider episodes from the last 7 days as current
powercrust --days 7 subscriptions.opml

# Sort episodes chronologically in the output file
powercrust --chronological true subscriptions.opml

# Output newest episodes in Markdown format
powercrust --format md subscriptions.opml

# Output newest episodes in HTML format
powercrust --format html subscriptions.opml
```

## Output Files

The program generates two output files:

1. `<OPML_FILENAME>.txt`: Contains all media URLs from the feeds (one URL per line)
   - When `--chronological` is enabled, URLs are sorted by publication date (oldest first)
   - When `--chronological` is disabled, URLs are in no particular order

2. `newest.<FORMAT>`: Contains the newest episode from each feed
   - Format depends on the `--format` option (txt, md, or html)
   - For txt format: `Feed Name: Episode Title [Date] - URL`
   - For md format: Markdown structured document with headers and links
   - For html format: HTML document with styling for better presentation

## Configuration

The RSS Feed Scraper can be configured through command-line arguments. You can combine multiple options to customize the behavior according to your needs.

### Time Window Configuration

By default, the program only includes episodes published within the last 30 days. You can:

- Disable this filter with `--check-current false`
- Adjust the time window with `--days <NUMBER>`

### Output Formatting

- Choose how the newest episodes are formatted with `--format <FORMAT>`
- Sort all episodes chronologically with `--chronological true` (default is off)

## Notes

- The OPML file should follow standard format with `<outline>` elements containing `text` and `xmlUrl` attributes
- The program uses a regex to extract media URLs, so it might miss some URLs if they don't match the pattern
- For feeds that don't provide publication dates, episodes will be treated as if they have no date when filtering and sorting

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
