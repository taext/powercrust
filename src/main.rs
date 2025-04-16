use rayon::prelude::*;
use regex::Regex;
use reqwest::{Client, StatusCode};
use futures::future::join_all;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Semaphore as TokioSemaphore;
use chrono::{DateTime, Utc, Duration};
use rss::Channel;
use clap::{App, Arg};

#[derive(Clone)]
struct Episode {
    feed_name: String,
    title: String,
    pub_date: Option<DateTime<Utc>>,
    media_url: String,
}

#[tokio::main]
async fn main() {
    // Use clap for command-line argument parsing
    let matches = App::new("RSS Feed Scraper")
        .version("0.3.0")
        .about("Scrapes RSS feeds from an OPML file and extracts media URLs")
        .before_help("                   ▗ 
▛▌▛▌▌▌▌█▌▛▘▛▘▛▘▌▌▛▘▜▘
▙▌▙▌▚▚▘▙▖▌ ▙▖▌ ▙▌▄▌▐▖
▌                    ")
        .arg(
            Arg::with_name("opml_file")
                .help("Path to the OPML file containing RSS feeds")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("check_current")
                .short('c')
                .long("check-current")
                .help("Only include episodes published within the last 30 days in newest output")
                .takes_value(true)
                .default_value("true"),
        )
        .arg(
            Arg::with_name("filter_all")
                .short('a')
                .long("filter-all")
                .help("Apply time filter to all_files output as well (not just newest)")
                .takes_value(true)
                .default_value("false"),
        )
        .arg(
            Arg::with_name("days")
                .short('d')
                .long("days")
                .help("Number of days to consider as current")
                .takes_value(true)
                .default_value("30"),
        )
        .arg(
            Arg::with_name("chronological")
                .short('o')
                .long("chronological")
                .help("Sort all episodes chronologically (oldest first) in the output file")
                .takes_value(true)
                .default_value("false"),
        )
        .arg(
            Arg::with_name("formats")
                .short('f')
                .long("formats")
                .help("Format(s) for output files (txt, md, html), comma-separated")
                .takes_value(true)
                .default_value("txt")
                .use_delimiter(true)
                .require_delimiter(true)
                .value_delimiter(','),
        )
        .arg(
            Arg::with_name("all_files_format")
                .short('F')
                .long("all-format")
                .help("Format for all_files output (txt, md, html)")
                .takes_value(true)
                .default_value("txt")
                .possible_values(&["txt", "md", "html"]),
        )
        .get_matches();

    let opml_path = PathBuf::from(matches.value_of("opml_file").unwrap());
    
    // Parse formats for newest files
    let formats: Vec<&str> = matches.values_of("formats").unwrap_or_default().collect();
    
    // Get format for all_files
    let all_files_format = matches.value_of("all_files_format").unwrap_or("txt");
    let output_txt = opml_path.with_extension(all_files_format);
    
    let check_current = matches.value_of("check_current")
        .unwrap_or("true")
        .to_lowercase() == "true";
    let filter_all = matches.value_of("filter_all")
        .unwrap_or("false")
        .to_lowercase() == "true";
    let current_days = matches
        .value_of("days")
        .unwrap()
        .parse::<i64>()
        .unwrap_or(30);
    
    let chronological = matches.value_of("chronological")
        .unwrap_or("false")
        .to_lowercase() == "true";

    let feeds = parse_opml(&opml_path);
    println!("Found {} feeds", feeds.len());

    let client = Arc::new(Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64)")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap());

    let sem = Arc::new(TokioSemaphore::new(20)); // Limit concurrent HTTP requests
    let fetches = join_all(
        feeds.into_iter().map(|(name, url)| {
            let client = Arc::clone(&client);
            let sem = Arc::clone(&sem);
            tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                fetch_feed(&client, &name, &url).await
            })
        }),
    )
    .await;

    // Collect raw XML results for backward compatibility
    let raw_results: Vec<(String, String)> = fetches
        .into_iter()
        .filter_map(|res| res.ok().flatten())
        .collect();

    // Extract all episodes with dates for chronological sorting (if enabled)
    let media_regex = Regex::new(r#""(http\S+?\.(mp3|mp4))["?]"#).unwrap();
    
    // Process all episodes - handle the two different approaches based on chronological flag
    if chronological {
        // Process feeds using structured approach with proper date filtering
        let mut all_episodes: Vec<Episode> = Vec::new();
        
        for (feed_name, content) in &raw_results {
            if let Ok(channel) = Channel::read_from(content.as_bytes()) {
                for item in channel.items() {
                    // Find media URL
                    let media_url = if let Some(enclosure) = item.enclosure() {
                        // First try the enclosure
                        enclosure.url.clone()
                    } else {
                        // Fall back to regex search in item description or content
                        let description = item.description().unwrap_or_default();
                        let content_encoded = item.extensions().get("content")
                            .and_then(|c| c.get("encoded"))
                            .and_then(|e| e.first())
                            .and_then(|v| v.value.as_deref())
                            .unwrap_or_default();
                        
                        let combined_content = format!("{} {}", description, content_encoded);
                        
                        if let Some(cap) = media_regex.captures(&combined_content) {
                            if let Some(url) = cap.get(1).map(|m| m.as_str().to_owned()) {
                                url
                            } else {
                                continue; // Skip if no media URL found
                            }
                        } else {
                            continue; // Skip if no media URL found
                        }
                    };
                    
                    // Parse publication date
                    let pub_date = item.pub_date().and_then(|date_str| {
                        DateTime::parse_from_rfc2822(date_str)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    });
                    
                    // Skip if we're checking for currency, filter_all is true,
                    // and the episode is too old
                    if check_current && filter_all {
                        if let Some(date) = pub_date {
                            let cutoff_date = Utc::now() - Duration::days(current_days);
                            if date < cutoff_date {
                                continue;
                            }
                        } else {
                            // If no date is available and we're checking for currency, skip
                            continue;
                        }
                    }
                    
                    all_episodes.push(Episode {
                        feed_name: feed_name.clone(),
                        title: item.title().unwrap_or("Unknown").to_string(),
                        pub_date,
                        media_url,
                    });
                }
            }
        }
        
        // Sort episodes chronologically (oldest first)
        all_episodes.sort_by(|a, b| {
            match (&a.pub_date, &b.pub_date) {
                (Some(a_date), Some(b_date)) => a_date.cmp(b_date),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        
        // Write all episodes to file in the requested format
        write_episodes_to_file(
            &all_episodes, 
            &output_txt, 
            all_files_format, 
            "All Podcast Episodes"
        );
        
        println!("Found {} episodes.", all_episodes.len());
        
        // Process feeds to extract newest episodes - only filter these by date in all cases
        let mut newest_episodes_map: HashMap<String, Episode> = HashMap::new();
        
        // Filter a copy of all_episodes for newest collection
        let filtered_episodes: Vec<Episode> = all_episodes
            .iter()
            .filter(|episode| {
                if !check_current {
                    return true;
                }
                if let Some(date) = episode.pub_date {
                    let cutoff_date = Utc::now() - Duration::days(current_days);
                    date >= cutoff_date
                } else {
                    false
                }
            })
            .cloned()
            .collect();
        
        for episode in filtered_episodes {
            newest_episodes_map.entry(episode.feed_name.clone())
                .or_insert(episode);
        }
        
        let newest_episodes: Vec<Episode> = newest_episodes_map.into_values().collect();
        
        // Write newest episodes in each requested format
        for format in &formats {
            let newest_path = opml_path.parent().unwrap().join(format!("newest.{}", format));
            write_episodes_to_file(
                &newest_episodes, 
                &newest_path, 
                format, 
                "Newest Podcast Episodes"
            );
        }
    } else {
        // Original functionality - extract using regex for all files
        // This preserves backward compatibility with the original approach
        let mut media_urls: Vec<Episode> = Vec::new();
        
        for (feed_name, content) in &raw_results {
            // First extract through RSS for structured data
            if let Ok(channel) = Channel::read_from(content.as_bytes()) {
                for item in channel.items() {
                    if let Some(enclosure) = item.enclosure() {
                        let pub_date = item.pub_date().and_then(|date_str| {
                            DateTime::parse_from_rfc2822(date_str)
                                .ok()
                                .map(|dt| dt.with_timezone(&Utc))
                        });
                        
                        // Only filter by date if filter_all is true
                        if check_current && filter_all {
                            if let Some(date) = pub_date {
                                let cutoff_date = Utc::now() - Duration::days(current_days);
                                if date < cutoff_date {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }
                        
                        media_urls.push(Episode {
                            feed_name: feed_name.clone(),
                            title: item.title().unwrap_or("Unknown").to_string(),
                            pub_date,
                            media_url: enclosure.url.clone(),
                        });
                    }
                }
            }
            
            // Also search for URLs with regex
            let mut feed_urls: Vec<String> = Vec::new();
            for cap in media_regex.captures_iter(content) {
                if let Some(url) = cap.get(1).map(|m| m.as_str().to_owned()) {
                    if !feed_urls.contains(&url) {
                        feed_urls.push(url.clone());
                        // For URLs found with regex, we don't have structured data
                        media_urls.push(Episode {
                            feed_name: feed_name.clone(),
                            title: "Unknown".to_string(),
                            pub_date: None,
                            media_url: url,
                        });
                    }
                }
            }
        }
        
        // Write all media URLs to output file in the requested format
        write_episodes_to_file(
            &media_urls, 
            &output_txt, 
            all_files_format, 
            "All Podcast Episodes"
        );
        
        println!("Found {} episodes with media URLs.", media_urls.len());
        
        // Process feeds to extract newest episodes - always filter these by date
        let mut all_episodes: Vec<Episode> = Vec::new();
        
        for (feed_name, content) in &raw_results {
            if let Ok(channel) = Channel::read_from(content.as_bytes()) {
                for item in channel.items() {
                    // Find media URL
                    let media_url = if let Some(enclosure) = item.enclosure() {
                        // First try the enclosure
                        enclosure.url.clone()
                    } else {
                        // Fall back to regex search in item description or content
                        let description = item.description().unwrap_or_default();
                        let content_encoded = item.extensions().get("content")
                            .and_then(|c| c.get("encoded"))
                            .and_then(|e| e.first())
                            .and_then(|v| v.value.as_deref())
                            .unwrap_or_default();
                        
                        let combined_content = format!("{} {}", description, content_encoded);
                        
                        if let Some(cap) = media_regex.captures(&combined_content) {
                            if let Some(url) = cap.get(1).map(|m| m.as_str().to_owned()) {
                                url
                            } else {
                                continue; // Skip if no media URL found
                            }
                        } else {
                            continue; // Skip if no media URL found
                        }
                    };
                    
                    // Parse publication date
                    let pub_date = item.pub_date().and_then(|date_str| {
                        DateTime::parse_from_rfc2822(date_str)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    });
                    
                    // Only keep episodes from the last N days if check_current is enabled
                    if check_current {
                        if let Some(date) = pub_date {
                            let cutoff_date = Utc::now() - Duration::days(current_days);
                            if date < cutoff_date {
                                continue;
                            }
                        } else {
                            // If no date is available and we're checking for currency, skip
                            continue;
                        }
                    }
                    
                    all_episodes.push(Episode {
                        feed_name: feed_name.clone(),
                        title: item.title().unwrap_or("Unknown").to_string(),
                        pub_date,
                        media_url,
                    });
                }
            }
        }
        
        // For newest episodes, collect the newest per feed
        let mut newest_episodes_map: HashMap<String, Episode> = HashMap::new();
        
        for episode in &all_episodes {
            let entry = newest_episodes_map.entry(episode.feed_name.clone());
            match entry {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(episode.clone());
                },
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    if let (Some(existing_date), Some(new_date)) = (e.get().pub_date, episode.pub_date) {
                        if new_date > existing_date {
                            e.insert(episode.clone());
                        }
                    }
                }
            }
        }
        
        let newest_episodes: Vec<Episode> = newest_episodes_map.into_values().collect();
        
        // Write newest episodes in each requested format
        for format in &formats {
            let newest_path = opml_path.parent().unwrap().join(format!("newest.{}", format));
            write_episodes_to_file(
                &newest_episodes, 
                &newest_path, 
                format, 
                "Newest Podcast Episodes"
            );
        }
    }

    println!("Done. All episodes written to {}.", output_txt.display());
    for format in &formats {
        let newest_path = opml_path.parent().unwrap().join(format!("newest.{}", format));
        println!("Newest episodes written to {}.", newest_path.display());
    }
}

// Helper function to write episodes to a file in the specified format
fn write_episodes_to_file(episodes: &[Episode], path: &PathBuf, format: &str, title: &str) {
    match format {
        "md" => {
            // Write in Markdown format
            let mut file = File::create(path).unwrap();
            
            writeln!(file, "# {}", title).unwrap();
            writeln!(file).unwrap();
            
            for episode in episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(
                    file,
                    "## {}\n\n**{}** [{}]\n\n[Listen]({})  \n",
                    episode.feed_name,
                    episode.title,
                    date_str,
                    episode.media_url
                ).unwrap();
            }
        },
        "html" => {
            // Write in HTML format
            let mut file = File::create(path).unwrap();
            
            writeln!(file, "<!DOCTYPE html>").unwrap();
            writeln!(file, "<html>").unwrap();
            writeln!(file, "<head>").unwrap();
            writeln!(file, "    <meta charset=\"UTF-8\">").unwrap();
            writeln!(file, "    <title>{}</title>", title).unwrap();
            writeln!(file, "    <style>").unwrap();
            writeln!(file, "        body {{ font-family: Arial, sans-serif; margin: 20px; }}").unwrap();
            writeln!(file, "        h1 {{ color: #333; }}").unwrap();
            writeln!(file, "        .episode {{ margin-bottom: 30px; border-bottom: 1px solid #eee; padding-bottom: 20px; }}").unwrap();
            writeln!(file, "        .feed-name {{ font-size: 1.5em; color: #2c3e50; margin-bottom: 5px; }}").unwrap();
            writeln!(file, "        .episode-title {{ font-weight: bold; font-size: 1.2em; }}").unwrap();
            writeln!(file, "        .date {{ color: #7f8c8d; margin-bottom: 10px; }}").unwrap();
            writeln!(file, "        .media-link {{ margin-top: 10px; }}").unwrap();
            writeln!(file, "        .media-link a {{ color: #3498db; text-decoration: none; }}").unwrap();
            writeln!(file, "        .media-link a:hover {{ text-decoration: underline; }}").unwrap();
            writeln!(file, "    </style>").unwrap();
            writeln!(file, "</head>").unwrap();
            writeln!(file, "<body>").unwrap();
            writeln!(file, "    <h1>{}</h1>", title).unwrap();
            
            for episode in episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(file, "    <div class=\"episode\">").unwrap();
                writeln!(file, "        <div class=\"feed-name\">{}</div>", html_escape(&episode.feed_name)).unwrap();
                writeln!(file, "        <div class=\"episode-title\">{}</div>", html_escape(&episode.title)).unwrap();
                writeln!(file, "        <div class=\"date\">{}</div>", date_str).unwrap();
                writeln!(file, "        <div class=\"media-link\"><a href=\"{}\">Listen</a></div>", episode.media_url).unwrap();
                writeln!(file, "    </div>").unwrap();
            }
            
            writeln!(file, "</body>").unwrap();
            writeln!(file, "</html>").unwrap();
        },
        _ => {
            // Default plain text format
            let mut file = File::create(path).unwrap();
            for episode in episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(
                    file,
                    "{}: {} [{}] - {}",
                    episode.feed_name,
                    episode.title,
                    date_str,
                    episode.media_url
                ).unwrap();
            }
        }
    }
}

fn parse_opml(path: &PathBuf) -> Vec<(String, String)> {
    let file = File::open(path).expect("Cannot open OPML file");
    let reader = BufReader::new(file);

    let text_re = Regex::new(r#"text="([^"]+)""#).unwrap();
    let url_re = Regex::new(r#"xmlUrl="([^"]+)""#).unwrap();

    let mut feeds = vec![];
    let mut found_first = false;

    for line in reader.lines().filter_map(Result::ok) {
        if line.contains("xmlUrl=") {
            let name = text_re.captures(&line).and_then(|c| c.get(1)).map(|m| m.as_str());
            let url = url_re.captures(&line).and_then(|c| c.get(1)).map(|m| m.as_str());

            if let (Some(n), Some(u)) = (name, url) {
                if !found_first {
                    found_first = true;
                    continue;
                }
                feeds.push((n.to_string(), u.to_string()));
            }
        }
    }
    feeds
}

async fn fetch_feed(client: &Client, name: &str, url: &str) -> Option<(String, String)> {
    match client.get(url).send().await {
        Ok(resp) => {
            if resp.status() == StatusCode::OK {
                if let Ok(content) = resp.text().await {
                    return Some((name.to_string(), content));
                }
            }
            None
        }
        Err(_) => None,
    }
}

fn html_escape(s: &str) -> String {
    s.replace("&", "&amp;")
     .replace("<", "&lt;")
     .replace(">", "&gt;")
     .replace("\"", "&quot;")
     .replace("'", "&#39;")
}
