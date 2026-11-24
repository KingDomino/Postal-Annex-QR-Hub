//! "What's New?" changelog modal dialog with lightweight markdown rendering and a
//! bottom pillbox "Close" button.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Id, Label, Layout, Margin, Modal, RichText,
    ScrollArea, Stroke, Ui,
};

use postal_annex_qr_scanner::updater::{VersionChangelog, CURRENT_VERSION};

use super::app::ScannerApp;
use super::colors::*;
use super::{
    close_icon_button, flat_button, font_bold, font_regular, font_semibold, modal_frame,
    ButtonStyle, MODAL_WIDTH,
};

/// A parsed line from a GitHub Release markdown changelog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangelogLine {
    Heading(String),
    Bullet(String),
    Paragraph(String),
}

/// Parses lightweight release-note markdown into headings, bullets, and paragraphs.
pub(crate) fn parse_changelog_lines(markdown: &str) -> Vec<ChangelogLine> {
    let mut items = Vec::new();
    for raw in markdown.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = strip_markdown_heading(trimmed) {
            if !heading.is_empty() {
                items.push(ChangelogLine::Heading(heading.to_owned()));
            }
        } else if let Some(bullet) = strip_markdown_bullet(trimmed) {
            if !bullet.is_empty() {
                items.push(ChangelogLine::Bullet(bullet.to_owned()));
            }
        } else {
            items.push(ChangelogLine::Paragraph(trimmed.to_owned()));
        }
    }
    items
}

fn strip_markdown_bullet(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix('-')
        .or_else(|| line.strip_prefix('*'))
        .or_else(|| line.strip_prefix('\u{2022}'))?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

fn strip_markdown_heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 {
        return None;
    }
    let rest = &line[hashes..];
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Renders the centered "What's New?" modal dialog over a click-blocking backdrop scrim.
pub(super) fn show(app: &mut ScannerApp, ctx: &egui::Context) {
    let mut should_close = false;
    let max_body_height = (ctx.screen_rect().height() - 210.0).clamp(140.0, 380.0);

    let modal_response = Modal::new(Id::new("whats_new_modal"))
        .backdrop_color(Color32::from_black_alpha(110))
        .frame(modal_frame())
        .show(ctx, |ui| {
            ui.set_width(MODAL_WIDTH - 40.0);
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            // Header bar: title + version pill + close icon button.
            ui.horizontal(|ui| {
                ui.add(
                    Label::new(
                        RichText::new("What's New?")
                            .font(font_bold(18.0))
                            .color(NAVY),
                    )
                    .selectable(false),
                );

                version_pill(ui, CURRENT_VERSION);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_icon_button(ui, 26.0, MUTED, GRAY_150)
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        should_close = true;
                    }
                });
            });

            ui.add(egui::Separator::default().spacing(8.0));

            ScrollArea::vertical()
                .id_salt("whats_new_scroll")
                .max_height(max_body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 8.0);
                    render_versions(ui, &app.whats_new_entries);
                });

            ui.add_space(6.0);

            // Prominent bottom pillbox "Close" button.
            let close_style = ButtonStyle::solid(NAVY, NAVY_HOVER, WHITE).with_radius(18);
            if flat_button(
                ui,
                egui::vec2(ui.available_width(), 36.0),
                "Close",
                font_bold(14.0),
                close_style,
            )
            .clicked()
            {
                should_close = true;
            }
        });

    if should_close || modal_response.should_close() {
        app.close_whats_new();
    }
}

fn version_pill(ui: &mut Ui, version: &str) {
    let label = version.trim().trim_start_matches(['v', 'V']);
    Frame::new()
        .fill(READY_BG)
        .stroke(Stroke::new(1.0_f32, READY_BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.add(
                Label::new(
                    RichText::new(format!("v{label}"))
                        .font(font_bold(11.5))
                        .color(NAVY),
                )
                .selectable(false),
            );
        });
}

fn render_versions(ui: &mut Ui, entries: &[VersionChangelog]) {
    let show_version_headers = entries.len() > 1;
    for (idx, entry) in entries.iter().enumerate() {
        if idx > 0 {
            ui.add_space(6.0);
            ui.add(egui::Separator::default().spacing(6.0));
            ui.add_space(2.0);
        }
        if show_version_headers {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                version_pill(ui, &entry.version);
                ui.add(
                    Label::new(
                        RichText::new(format!(
                            "Changes in v{}",
                            entry.version.trim().trim_start_matches(['v', 'V'])
                        ))
                        .font(font_semibold(13.5))
                        .color(NAVY),
                    )
                    .selectable(false),
                );
            });
        }
        let parsed = parse_changelog_lines(&entry.notes);
        render_changelog(ui, &parsed);
    }
}

fn render_changelog(ui: &mut Ui, lines: &[ChangelogLine]) {
    for (idx, line) in lines.iter().enumerate() {
        match line {
            ChangelogLine::Heading(text) => {
                if idx > 0 {
                    ui.add_space(4.0);
                }
                ui.add(
                    Label::new(RichText::new(text).font(font_bold(13.5)).color(NAVY))
                        .wrap()
                        .selectable(false),
                );
            }
            ChangelogLine::Bullet(text) => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add(
                        Label::new(
                            RichText::new("\u{2022}")
                                .font(font_bold(13.5))
                                .color(NAVY),
                        )
                        .selectable(false),
                    );
                    ui.add(
                        Label::new(
                            RichText::new(text)
                                .font(font_regular(13.0))
                                .color(TEXT_BODY),
                        )
                        .wrap()
                        .selectable(false),
                    );
                });
            }
            ChangelogLine::Paragraph(text) => {
                ui.add(
                    Label::new(
                        RichText::new(text)
                            .font(font_regular(13.0))
                            .color(TEXT_BODY),
                    )
                    .wrap()
                    .selectable(false),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_bullets_and_paragraphs() {
        let md = "\
# Release v2.0.2

## Highlights
- Added a \"What's New?\" dialog
* Added a footer issue button
\u{2022} Seamless window chrome

### Notes
Plain paragraph line.
   
-   Trimmed bullet  
";

        assert_eq!(
            parse_changelog_lines(md),
            vec![
                ChangelogLine::Heading("Release v2.0.2".to_string()),
                ChangelogLine::Heading("Highlights".to_string()),
                ChangelogLine::Bullet("Added a \"What's New?\" dialog".to_string()),
                ChangelogLine::Bullet("Added a footer issue button".to_string()),
                ChangelogLine::Bullet("Seamless window chrome".to_string()),
                ChangelogLine::Heading("Notes".to_string()),
                ChangelogLine::Paragraph("Plain paragraph line.".to_string()),
                ChangelogLine::Bullet("Trimmed bullet".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_empty_lines_and_keeps_hashtag_words_as_paragraphs() {
        let md = "\n   \n#hashtag is a paragraph\n##   \n-   \n";
        assert_eq!(
            parse_changelog_lines(md),
            vec![ChangelogLine::Paragraph("#hashtag is a paragraph".to_string())]
        );
    }
}
