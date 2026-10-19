//! Searches the web, or reads a page, with the curl adapter and prints
//! what the model would get.
//!
//! `cargo run -p adapter-web --example search -- "rust 1.90 release"`
//! `cargo run -p adapter-web --example search -- --fetch https://www.rust-lang.org/`
//!
//! The backend comes from `CREST_WEB_BACKEND` (`duckduckgo`, the default;
//! `searxng` with `CREST_SEARXNG_URL`; `brave` with `BRAVE_API_KEY`).

use adapter_web::CurlWeb;
use application::ports::web::WebPort as _;
use application::web::{SearchBackend, WebOptions};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = match std::env::var("CREST_WEB_BACKEND").as_deref() {
        Ok("brave") => SearchBackend::Brave,
        Ok("searxng") => SearchBackend::Searxng {
            url: std::env::var("CREST_SEARXNG_URL").unwrap_or_default(),
        },
        _ => SearchBackend::DuckDuckGo,
    };
    let options = WebOptions {
        backend,
        ..WebOptions::default()
    };
    let web = CurlWeb::new(options, std::env::var("BRAVE_API_KEY").ok());
    let result = match args.as_slice() {
        [flag, url] if flag == "--fetch" => web.fetch(url).await.map(|page| {
            format!(
                "Source: {}\nTitle: {}\n\n{}",
                page.url,
                page.title.unwrap_or_default(),
                page.text
            )
        }),
        [] => {
            eprintln!("usage: search <query> | search --fetch <url>");
            std::process::exit(2);
        }
        words => web.search(&words.join(" "), 8).await.map(|hits| {
            hits.iter()
                .enumerate()
                .map(|(ix, hit)| {
                    format!("[{}] {}\n{}\n{}\n", ix + 1, hit.title, hit.url, hit.snippet)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
    };
    match result {
        Ok(text) => println!("{text}"),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
