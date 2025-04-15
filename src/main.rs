use rayon::prelude::*;
use regex::Regex;
use reqwest::{Client, StatusCode};
use futures::future::join_all;
use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Semaphore as TokioSemaphore;
use chrono::{DateTime, Utc, Duration};
use rss::Channel;
use clap::{App, Arg};

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
                .help("Only include episodes published within the last 30 days")
                .takes_value(true)
                .default_value("true"),
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
            Arg::with_name("newest_format")
                .short('f')
                .long("format")
                .help("Format for newest.txt output (txt, md, html)")
                .takes_value(true)
                .default_value("txt")
                .possible_values(&["txt", "md", "html"]),
        )
        .get_matches();

    let opml_path = PathBuf::from(matches.value_of("opml_file").unwrap());
    let output_txt = opml_path.with_extension("txt");
    
    // Determine the newest output file extension based on format
    let newest_format = matches.value_of("newest_format").unwrap_or("txt");
    let newest_path = opml_path.parent().unwrap().join(format!("newest.{}", newest_format));
    
    let check_current = matches.value_of("check_current")
        .unwrap_or("true")
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
    
    if chronological {
        // Extract episodes with dates for chronological sorting
        let mut all_episodes: Vec<Episode> = Vec::new();
        
        for (feed_name, content) in &raw_results {
            if let Ok(channel) = Channel::read_from(content.as_bytes()) {
                for item in channel.items() {
                    // Find media URL using same logic as for newest episodes
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
                    
                    // Skip if we're checking for currency and the episode is too old
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
        
        // Sort episodes chronologically (oldest first)
        all_episodes.sort_by(|a, b| {
            match (&a.pub_date, &b.pub_date) {
                (Some(a_date), Some(b_date)) => a_date.cmp(b_date),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        
        // Write all media URLs to output file in chronological order
        let mut file = File::create(output_txt).unwrap();
        for episode in all_episodes {
            writeln!(file, "{}", episode.media_url).unwrap();
        }
    } else {
        // Original functionality (random order, just extracting URLs)
        let media_urls: HashSet<String> = raw_results
            .par_iter()
            .flat_map(|(_, content)| {
                media_regex
                    .captures_iter(content)
                    .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_owned()))
                    .collect::<Vec<_>>()
            })
            .collect();

        // Write all media URLs to output file (original functionality)
        let mut file = File::create(output_txt).unwrap();
        for url in media_urls {
            writeln!(file, "{}", url).unwrap();
        }
    }

    // Process feeds to extract newest episodes
    let mut newest_episodes: Vec<Episode> = Vec::new();
    
    for (feed_name, content) in &raw_results {
        if let Ok(channel) = Channel::read_from(content.as_bytes()) {
            // Find newest episode for this feed
            let mut feed_episodes = Vec::new();
            
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
                
                // Skip if we're checking for currency and the episode is too old
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
                
                feed_episodes.push(Episode {
                    feed_name: feed_name.clone(),
                    title: item.title().unwrap_or("Unknown").to_string(),
                    pub_date,
                    media_url,
                });
            }
            
            // Sort episodes by date (newest first)
            feed_episodes.sort_by(|a, b| {
                match (&b.pub_date, &a.pub_date) {
                    (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
            
            // Take the newest episode
            if let Some(newest) = feed_episodes.first() {
                newest_episodes.push(Episode {
                    feed_name: newest.feed_name.clone(),
                    title: newest.title.clone(),
                    pub_date: newest.pub_date,
                    media_url: newest.media_url.clone(),
                });
            }
        }
    }
    
    // Write newest episodes in the requested format
    match newest_format {
        "md" => {
            // Write in Markdown format
            let mut newest_file = File::create(&newest_path).unwrap();
            
            writeln!(newest_file, "# Newest Podcast Episodes").unwrap();
            writeln!(newest_file).unwrap();
            
            for episode in newest_episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(
                    newest_file,
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
            let mut newest_file = File::create(&newest_path).unwrap();
            
            writeln!(newest_file, "<!DOCTYPE html>").unwrap();
            writeln!(newest_file, "<html>").unwrap();
            writeln!(newest_file, "<head>").unwrap();
            writeln!(newest_file, "    <meta charset=\"UTF-8\">").unwrap();
            writeln!(newest_file, "    <title>Newest Podcast Episodes</title>").unwrap();
            writeln!(newest_file, "    <style>").unwrap();
            writeln!(newest_file, "        body {{ font-family: Arial, sans-serif; margin: 20px; }}").unwrap();
            writeln!(newest_file, "        h1 {{ color: #333; }}").unwrap();
            writeln!(newest_file, "        .episode {{ margin-bottom: 30px; border-bottom: 1px solid #eee; padding-bottom: 20px; }}").unwrap();
            writeln!(newest_file, "        .feed-name {{ font-size: 1.5em; color: #2c3e50; margin-bottom: 5px; }}").unwrap();
            writeln!(newest_file, "        .episode-title {{ font-weight: bold; font-size: 1.2em; }}").unwrap();
            writeln!(newest_file, "        .date {{ color: #7f8c8d; margin-bottom: 10px; }}").unwrap();
            writeln!(newest_file, "        .media-link {{ margin-top: 10px; }}").unwrap();
            writeln!(newest_file, "        .media-link a {{ color: #3498db; text-decoration: none; }}").unwrap();
            writeln!(newest_file, "        .media-link a:hover {{ text-decoration: underline; }}").unwrap();
            writeln!(newest_file, "    </style>").unwrap();
            writeln!(newest_file, "</head>").unwrap();
            writeln!(newest_file, "<body>").unwrap();
            writeln!(newest_file, "    <h1>Newest Podcast Episodes</h1>").unwrap();
            
            for episode in newest_episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(newest_file, "    <div class=\"episode\">").unwrap();
                writeln!(newest_file, "        <div class=\"feed-name\">{}</div>", html_escape(&episode.feed_name)).unwrap();
                writeln!(newest_file, "        <div class=\"episode-title\">{}</div>", html_escape(&episode.title)).unwrap();
                writeln!(newest_file, "        <div class=\"date\">{}</div>", date_str).unwrap();
                writeln!(newest_file, "        <div class=\"media-link\"><a href=\"{}\">Listen</a></div>", episode.media_url).unwrap();
                writeln!(newest_file, "    </div>").unwrap();
            }
            
            writeln!(newest_file, "</body>").unwrap();
            writeln!(newest_file, "</html>").unwrap();
        },
        _ => {
            // Default plain text format
            let mut newest_file = File::create(&newest_path).unwrap();
            for episode in newest_episodes {
                let date_str = episode.pub_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "Unknown date".to_string());
                    
                writeln!(
                    newest_file,
                    "{}: {} [{}] - {}",
                    episode.feed_name,
                    episode.title,
                    date_str,
                    episode.media_url
                ).unwrap();
            }
        }
    }

    println!("Done. All media URLs written to output file.");
    println!("Newest episodes written to {}.", newest_path.display());
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
