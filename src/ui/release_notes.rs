use iced::widget::markdown;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// GitHub release notes commonly contain bare URLs, which iced does not autolink.
pub fn parse(source: &str) -> Vec<markdown::Item> {
    markdown::parse(&autolink(source)).collect()
}

fn autolink(source: &str) -> String {
    let mut insertions = Vec::new();
    let mut excluded = 0;
    let mut text_ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(source, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)) => excluded += 1,
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => excluded -= 1,
            Event::Text(_) if excluded == 0 => {
                if let Some(previous) = text_ranges
                    .last_mut()
                    .filter(|previous| previous.end == range.start)
                {
                    previous.end = range.end;
                } else {
                    text_ranges.push(range);
                }
            }
            _ => {}
        }
    }
    for range in text_ranges {
        let raw = &source[range.clone()];
        let mut offset = 0;
        while let Some(start) = raw[offset..].find("http") {
            let start = offset + start;
            offset = start + 4;
            let candidate = &raw[start..];
            if !candidate.starts_with("https://") && !candidate.starts_with("http://") {
                continue;
            }
            if start > 0 && raw[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let end = candidate.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`')).unwrap_or(candidate.len());
            let mut url = candidate[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            // Keep balanced parentheses in URLs, but not prose punctuation.
            while url.ends_with(')') && url.matches(')').count() > url.matches('(').count() {
                url = &url[..url.len() - 1];
            }
            if reqwest::Url::parse(url).is_ok_and(|url| url.host_str().is_some()) {
                insertions.push(range.start + start..range.start + start + url.len());
            }
            offset = start + end;
        }
    }
    let mut result = String::with_capacity(source.len() + insertions.len() * 2);
    let mut offset = 0;
    for range in insertions {
        result.push_str(&source[offset..range.start]);
        result.push('<');
        result.push_str(&source[range.clone()]);
        result.push('>');
        offset = range.end;
    }
    result.push_str(&source[offset..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_release_url_becomes_a_markdown_link() {
        let source = "**Full Changelog**: https://github.com/VitalyArt/AirPods-Linux-Driver/commits/v1.0.0";
        let items = parse(source);
        let markdown::Item::Paragraph(text) = &items[0] else { panic!("expected paragraph") };
        let spans = text.spans(markdown::Style::from_palette(iced::Theme::Light.palette()));
        assert!(spans.iter().any(|span| span.link.as_deref() == Some("https://github.com/VitalyArt/AirPods-Linux-Driver/commits/v1.0.0")));
    }

    #[test]
    fn preserves_existing_links_images_and_code() {
        let source = "[https://label.test](https://target.test) <https://auto.test> ![https://alt.test](https://image.test) `https://code.test`\n\n```\nhttps://block.test\n```";
        assert_eq!(autolink(source), source);
    }

    #[test]
    fn handles_prose_unicode_and_multiple_urls() {
        assert_eq!(autolink("Ссылки: https://example.org/a_(b), (http://example.org/c)."),
            "Ссылки: <https://example.org/a_(b)>, (<http://example.org/c>).");
    }
}
