use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::knowledge::types::{KnowledgeSearchResult, KnowledgeStats};

pub fn format_search_results(results: &[KnowledgeSearchResult]) -> String {
    if results.is_empty() {
        return "No results found".to_string();
    }

    let mut output = String::new();

    for result in results {
        output.push_str(&"━".repeat(60));
        output.push('\n');

        // Source title
        output.push_str(&result.chunk.source_title.blue().bold().to_string());
        output.push('\n');

        // Source URL
        output.push_str(&result.chunk.source_url.bright_black().to_string());
        output.push('\n');

        // Section path
        if !result.chunk.section_path.is_empty() {
            output.push_str(&result.chunk.section_path.join(" > ").cyan().to_string());
            output.push('\n');
        }

        // Content preview (first 200 chars)
        let content = if result.chunk.content.chars().count() > 200 {
            format!("{}...", truncate_chars(&result.chunk.content, 200))
        } else {
            result.chunk.content.clone()
        };
        output.push_str(&content);
        output.push('\n');

        // Relevance score
        let score_pct = (result.relevance_score * 100.0) as u32;
        output.push_str(&format!("{}% relevant", score_pct).green().to_string());
        output.push_str("\n\n");
    }

    output
}

pub fn format_stats(stats: &KnowledgeStats) -> String {
    let mut output = String::new();

    output.push_str(&"Knowledge Base Statistics".bold().to_string());
    output.push('\n');
    output.push_str(&format!("Total Sources: {}", stats.total_sources));
    output.push('\n');
    output.push_str(&format!("Total Chunks: {}", stats.total_chunks));
    output.push('\n');

    if stats.total_sources > 0 {
        let avg = stats.total_chunks / stats.total_sources;
        output.push_str(&format!("Average Chunks/Source: {}", avg));
        output.push('\n');
    }

    if let Some(oldest) = stats.oldest_indexed {
        output.push_str(&format!("Oldest Indexed: {}", format_relative_time(oldest)));
        output.push('\n');
    }

    if let Some(newest) = stats.newest_indexed {
        output.push_str(&format!("Newest Indexed: {}", format_relative_time(newest)));
        output.push('\n');
    }

    output
}

pub fn format_source_list(sources: &[(String, String, usize, DateTime<Utc>)]) -> String {
    if sources.is_empty() {
        return "No sources indexed".to_string();
    }

    let mut output = String::new();

    // Header
    output.push_str(
        &format!(
            "{:<52} {:<32} {:<8} {}\n",
            "URL", "Title", "Chunks", "Last Indexed"
        )
        .bold()
        .to_string(),
    );
    output.push_str(&"─".repeat(120));
    output.push('\n');

    // Rows
    for (url, title, chunks, last_checked) in sources {
        let url_truncated = if url.len() > 50 {
            format!("{}...", truncate_chars(url, 47))
        } else {
            url.clone()
        };

        let title_truncated = if title.len() > 30 {
            format!("{}...", truncate_chars(title, 27))
        } else {
            title.clone()
        };

        output.push_str(&format!(
            "{:<52} {:<32} {:<8} {}\n",
            url_truncated,
            title_truncated,
            chunks,
            format_relative_time(*last_checked)
        ));
    }

    output
}

fn format_relative_time(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    if duration.num_days() > 0 {
        format!("{} days ago", duration.num_days())
    } else if duration.num_hours() > 0 {
        format!("{} hours ago", duration.num_hours())
    } else if duration.num_minutes() > 0 {
        format!("{} minutes ago", duration.num_minutes())
    } else {
        "just now".to_string()
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}
