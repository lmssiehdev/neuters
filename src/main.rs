mod api;

use api::{
    common::Articles, error::ApiError, search::fetch_articles_by_search,
    section::fetch_articles_by_section, topic::fetch_articles_by_topic,
};
use cached::proc_macro::{cached, once};
use chrono::{DateTime, Utc};
use maud::{html, PreEscaped, DOCTYPE};

use crate::api::{article::fetch_article, byline};

const CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/main.css"));

macro_rules! document {
    ($title:expr, $content:expr, $( $head:expr )? ) => {
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    title { ($title) }
                    style { (CSS) }
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    $( ($head) )?
                }
                body {
                    main { ($content) }
                    footer { div { a href="/" { "Home" } " - " a href="/about" { "About" } } }
                }
            }
        }
    };
}

fn main() {
    rouille::start_server("0.0.0.0:13369", move |request| {
        let path = request.url();
        let response = match path.as_str() {
            "/favicon.ico" => Response {
                code: 404,
                body: Body::Data("image/x-icon", vec![]),
                cache_time: 0,
            },
            "/" | "/home" => render_section("/home".to_string(), 8),
            "/about" => render_about(),
            "/search" | "/search/" => match request.get_param("query") {
                Some(query) => {
                    let offset = request
                        .get_param("offset")
                        .map_or(0, |s| s.parse::<u32>().unwrap_or(0));
                    render_search(&query, offset, 10)
                }
                None => Response {
                    code: 404,
                    body: Body::Data("text/html", vec![]),
                    cache_time: 0,
                },
            },
            _ => {
                if path.starts_with("/authors/") {
                    let offset = request
                        .get_param("offset")
                        .map_or(0, |s| s.parse::<u32>().unwrap_or(0));
                    render_topic(path, offset, 20)
                } else if path.starts_with("/article/") {
                    render_error(400, "Please disable forwards to this page.", &path)
                } else {
                    render_article(path)
                }
            }
        };

        match response.body {
            Body::Html(body) => rouille::Response::html(body),
            Body::Data(content_type, data) => rouille::Response::from_data(content_type, data),
        }
        .with_status_code(response.code)
        .with_public_cache(response.cache_time)
    });
}

// Wrappers that implement Clone so we can use them in the cache
// TODO: consider switching to nginx proxy cache
#[derive(Clone)]
struct Response {
    code: u16,
    body: Body,
    cache_time: u64,
}

#[derive(Clone)]
enum Body {
    Html(String),
    Data(&'static str, Vec<u8>),
}

#[cached(time = 86400)]
fn render_article(path: String) -> Response {
    let article = match fetch_article(&path) {
        Ok(article) => article,
        Err(err) => {
            return render_api_error(&err, &path);
        }
    };

    let published_time = article
        .published_time
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    let doc = document!(
        &article.title,
        html!(
            h1 { (&article.title) }
            p class="byline" {
                @let time = published_time.format("%Y-%m-%d %H:%M").to_string();
                @let byline = byline::render_byline(&article.authors);
                (time) " - " (PreEscaped(byline))
            }
            (render_items(&article.content_elements.unwrap_or_default()))
        ),
        html! {
            meta property="og:title" content=(&article.title);
            meta property="og:type" content="article";
            meta property="og:description" content=(&article.description);
            meta property="og:url" content=(path);
        }
    );

    Response {
        code: 200,
        body: Body::Html(doc.into_string()),
        cache_time: 24 * 60 * 60,
    }
}

fn render_items(items: &[serde_json::Value]) -> maud::Markup {
    html! {
        @for content in items {
            @match content["type"].as_str() {
                Some("header") => {
                    @if let Some(header) = content["content"].as_str() {
                        @match content["level"].as_u64().unwrap_or(0) {
                            0 => h1 { (header) },
                            1 => h2 { (header) },
                            _ => h3 { (header) },
                        }
                    }
                }
                Some("paragraph") => {
                    @if let Some(content) = content["content"].as_str() {
                        p { (PreEscaped(&content)) }
                    }
                }
                Some("image") => {
                    @if let Some(image) = content["url"].as_str() {
                        @let alt = content["alt"].as_str();
                        @let (width, height) = (content["width"].as_u64(), content["height"].as_u64());
                        img src=(image) alt=[alt] width=[width] height=[height];
                    }
                }
                Some("graphic") => {
                    @match content["graphic_type"].as_str() {
                        Some("image") => {
                            @if let (Some(image), Some(description)) = (content["url"].as_str(), content["description"].as_str()) {
                                figure {
                                    img src=(image) alt=(description);
                                    figcaption { (description) }
                                }
                            }
                        }
                        Some(unknown) => { p { "Unknown graphic type: " (unknown) } }
                        None => { p { "Missing graphic type" } }
                    }
                }
                Some("table") => {
                    @let rows = match content["rows"].as_array() { Some(rows) => rows, None => continue };
                    table {
                        thead {
                            @let row = match rows[0].as_array() { Some(row) => row, None => continue };
                            tr {
                                @for cell in row.iter() {
                                    th { (cell.as_str().unwrap_or_default()) }
                                }
                            }
                        }
                        tbody {
                            @for row in rows[1..].iter() {
                                tr {
                                    @let cells = match row.as_array() { Some(cells) => cells, None => continue };
                                    @for cell in cells {
                                        td { (PreEscaped(cell.as_str().unwrap_or_default())) }
                                    }
                                }
                            }
                        }
                    }
                }
                Some("list") => {
                    @if let Some(items) = content["items"].as_array() {
                        (render_items(items))
                    }
                }
                Some("social_media") => {
                    @if let Some(markup) = content["html"].as_str() {
                        @let embed = if let Some(index) = markup.find("\n<script") {
                            &markup[..index]
                        } else {
                            markup
                        };
                       (maud::PreEscaped(embed))
                    }
                }
                Some(unknown) => { p { "Unknown type: " (unknown) } }
                None => { p { "Failed to parse content element" } }
            }
        }
    }
}

#[cached(time = 3600)]
fn render_topic(path: String, offset: u32, size: u32) -> Response {
    render_articles(&path, fetch_articles_by_topic(&path, offset, size))
}

#[cached(time = 3600)]
fn render_section(path: String, size: u32) -> Response {
    render_articles(&path, fetch_articles_by_section(&path, size))
}

fn render_search(keywords: &str, offset: u32, size: u32) -> Response {
    render_articles("/search", fetch_articles_by_search(keywords, offset, size))
}

fn render_articles(path: &str, response: Result<Articles, ApiError>) -> Response {
    let articles = match response {
        Ok(articles) => articles,
        Err(err) => {
            return render_api_error(&err, path);
        }
    };

    let doc = document!(
        "Neuters - Reuters Proxy",
        html! {
            ul {
                @for article in articles.articles {
                    li { a href=(&article.canonical_url) { (&article.title) } }
                }
            }
        },
    );

    Response {
        code: 200,
        body: Body::Html(doc.into_string()),
        cache_time: 60 * 60,
    }
}

#[once]
fn render_about() -> Response {
    let doc = document!(
        "About",
        html! {
            h1 { "About" }
            p { "This is an alternative frontend to " a href="https://www.reuters.com/" { "Reuters" } ". It is intented to be lightweight, fast and was heavily inspired by " a href="https://nitter.net/" { "Nitter" } "." }
            ul {
                li { "No JavaScript or ads" }
                li { "No tracking" }
                li { "No cookies" }
                li { "Lightweight (usually <10KiB vs 50MiB from Reuters)" }
                li { "Dynamic Theming (respects system theme)" }
            }
            p { "You can install " a href="https://addons.mozilla.org/en-US/firefox/addon/reuters-redirect/" { "this browser extension" } " to automatically forwards all reuters links to this site." }
            p { "This is a work in progress. Please report any bugs or suggestions at " a href="https://github.com/HookedBehemoth/supreme-waffle" { "GitHub" } "." }

            h2 { "Contact" }
            p { "If you have any questions, feel free to contact me at " a href = "mailto:admin@boxcat.site" { "admin@boxcat.site" } "." }

            h2 { "Credits" }
            ul {
                li { a href="https://github.com/lambda-fairy/maud" { "maud" } ", a fast and intuitive inline html macro" }
                li { a href="https://github.com/jaemk/cached" { "cached" } ", a macro for caching responses" }
            }

            h2 { "License" }
            p { "This project is licensed under the " a href="https://www.gnu.org/licenses/licenses.html#AGPL" { "GNU Affero General Public License" } "." }
        },
    );

    Response {
        code: 200,
        body: Body::Html(doc.into_string()),
        cache_time: 24 * 60 * 60,
    }
}

fn render_error(code: u16, message: &str, path: &str) -> Response {
    let title = format!("{} - {}", code, message);

    let doc = document!(
        &title,
        html! {
            h1 { (&title) }
            p { "You tried to access \"" (path) "\"" }
            p { a href="/" { "Go home" } }
            p { a href=(path) { "Try again" } }
        },
    );

    Response {
        code,
        body: Body::Html(doc.into_string()),
        cache_time: 0,
    }
}

fn render_api_error(err: &ApiError, path: &str) -> Response {
    match &err {
        ApiError::External(code, message) => render_error(*code, message, path),
        ApiError::Internal(message) => render_error(500, message, path),
    }
}
