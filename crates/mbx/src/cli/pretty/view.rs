// Adapted from cargo-pretty's src/main.rs live and completed row rendering.
// Copyright (c) 2026 romancitodev (MIT); see LICENSE.cargo-pretty.
// Upstream revision: 73777522ccf8e4a485d026b3b60a275af2f8b381.
use super::model::{Model, segments};
use super::upstream::{fade, warning_panel};
use norimel::{self as rimel, Block, palette};

const BAR_WIDTH: usize = 28;
const NAME_WIDTH: usize = 32;
const ROWS: usize = 6;

#[derive(Default)]
pub(super) struct Browser {
    pub selected: usize,
    pub inspecting: bool,
    pub scroll: usize,
}

pub(super) fn render(
    model: &Model,
    browser: Option<&mut Browser>,
    width: u16,
    height: u16,
) -> Block {
    if let Some(browser) = browser {
        let block = if !model.warnings.is_empty() && browser.selected < model.warnings.len() {
            warning_panel(&model.warnings, browser.selected, browser.inspecting)
        } else {
            let index = browser.selected.saturating_sub(model.warnings.len());
            if let Some((name, details)) = model.failures.get(index) {
                rimel::col([
                    rimel::text(format!("✗ {name}")).fg(palette::RED).bold(),
                    rimel::text(if browser.inspecting {
                        details.clone()
                    } else {
                        "Enter inspect failure   ↑↓ navigate   Esc dismiss".into()
                    }),
                ])
            } else {
                summary(model)
            }
        };
        browser.scroll = browser
            .scroll
            .min(usize::from(block.size().1.saturating_sub(height)));
        return crop(block, browser.scroll, width, height);
    }
    if model.finished.is_some() {
        return crop(summary(model), 0, width, height);
    }
    let mut lines = vec![
        rimel::text(&model.command).dim(),
        rimel::separator(48).dim(),
        rimel::text(if model.testing {
            "Running tests (per-test start times unavailable)"
        } else {
            "Compiling"
        })
        .dim(),
    ];
    let rows = ROWS.min(usize::from(height.saturating_sub(12)) / 2).max(1);
    let count = model.live.len().min(rows);
    let mut live: Vec<_> = model.live.iter().collect();
    live.sort_by_key(|(_, start)| *start);
    for (i, (name, start)) in live.into_iter().take(rows).enumerate() {
        lines.push(fade(
            rimel::row([
                rimel::text("● ").fg(palette::YELLOW),
                rimel::text(format!("{name:<NAME_WIDTH$}")).fg(palette::TEXT),
                rimel::text(format!("{:05.2}s", start.elapsed().as_secs_f32()))
                    .fg(palette::SUBTEXT0),
            ]),
            (i + 1) as f32 / count.max(1) as f32,
        ));
    }
    for i in count..rows {
        lines.push(rimel::text(if i == 0 { "  …" } else { "" }).dim());
    }
    lines.push(rimel::separator(48).dim());
    lines.push(
        rimel::text(if model.testing {
            format!("Tests ({}/{})", model.suite_done, model.suite_total)
        } else {
            format!(
                "Compiled ({} artifacts, {} fresh)",
                model.artifacts, model.fresh
            )
        })
        .dim(),
    );
    let recent: Vec<_> = model.done.iter().rev().take(rows).collect();
    for _ in recent.len()..rows {
        lines.push(rimel::text(""));
    }
    let count = recent.len();
    for (i, row) in recent.into_iter().rev().enumerate() {
        let time = if row.fresh {
            "fresh".into()
        } else {
            row.duration
                .map_or("—".into(), |d| format!("{:05.2}s", d.as_secs_f32()))
        };
        lines.push(fade(
            rimel::row([
                rimel::text(if row.failed { "✗ " } else { "✓ " }).fg(if row.failed {
                    palette::RED
                } else {
                    palette::GREEN
                }),
                rimel::text(format!("{:<NAME_WIDTH$}", row.name)).fg(palette::TEXT),
                rimel::text(time).fg(palette::SUBTEXT0),
            ]),
            (i + 1) as f32 / count.max(1) as f32,
        ));
    }
    lines.push(rimel::text(""));
    if model.testing {
        let filled = BAR_WIDTH * model.suite_done / model.suite_total.max(1);
        lines.push(rimel::row([
            rimel::text("Tests         ").dim(),
            rimel::text("█".repeat(filled.min(BAR_WIDTH))).fg(palette::SAPPHIRE),
            rimel::text("░".repeat(BAR_WIDTH.saturating_sub(filled))).fg(palette::SURFACE1),
            rimel::text(format!("  {}/{}", model.suite_done, model.suite_total)),
        ]));
    }
    lines.extend(cache(model));
    lines.push(rimel::row([
        rimel::text("Elapsed       ").dim(),
        rimel::text(format!("{:.2}s", model.started.elapsed().as_secs_f32())),
    ]));
    if !model.warnings.is_empty() || !model.failures.is_empty() {
        lines.push(
            rimel::text(format!(
                "{} warnings · {} failed tests",
                model.warnings.len(),
                model.tests_failed
            ))
            .fg(palette::YELLOW),
        );
    }
    crop(rimel::col(lines), 0, width, height)
}

fn cache(model: &Model) -> Vec<Block> {
    let filled = if model.build_ok == Some(true) {
        BAR_WIDTH
    } else {
        model.units_total.map_or(BAR_WIDTH, |total| {
            BAR_WIDTH * model.units_done.min(total) / total.max(1)
        })
    };
    let [hits, misses, bypasses] = segments(&model.mix, filled);
    let label = match (model.build_ok, model.units_total) {
        (Some(true), Some(_)) => "build complete".into(),
        (Some(true), None) => "complete · total unknown".into(),
        (_, Some(total)) => format!("{}/{} units", model.units_done, total),
        (_, None) => "total unknown".into(),
    };
    vec![
        rimel::row([
            rimel::text("Build / cache ").dim(),
            rimel::text("█".repeat(hits)).fg(palette::GREEN),
            rimel::text("█".repeat(misses)).fg(palette::YELLOW),
            rimel::text("█".repeat(bypasses)).fg(palette::SUBTEXT0),
            rimel::text("░".repeat(BAR_WIDTH - filled)).fg(palette::SURFACE1),
            rimel::text(format!("  {label}")),
        ]),
        rimel::row([
            rimel::text(format!("{} hits  ", model.mix.hits)).fg(palette::GREEN),
            rimel::text(format!("{} misses  ", model.mix.misses)).fg(palette::YELLOW),
            rimel::text(format!(
                "{} bypassed · {} not looked up",
                model.mix.bypasses, model.mix.unconsulted
            ))
            .fg(palette::SUBTEXT0),
        ]),
        rimel::text(format!(
            "Estimated compiler time saved: {:.2}s",
            model.mix.saved_ns as f64 / 1e9
        ))
        .dim(),
    ]
}

pub(super) fn summary(model: &Model) -> Block {
    let (ok, elapsed) = model.finished.unwrap_or((true, model.started.elapsed()));
    let mut lines = vec![
        rimel::text(format!(
            "{} {} in {:.2}s",
            if ok { "✓" } else { "✗" },
            model.command,
            elapsed.as_secs_f32()
        ))
        .fg(if ok { palette::GREEN } else { palette::RED })
        .bold(),
        rimel::text(format!(
            "  {} artifacts compiled · {} fresh",
            model.artifacts, model.fresh
        ))
        .dim(),
    ];
    if model.testing {
        lines.push(rimel::text(format!(
            "  {} passed · {} failed · {} ignored",
            model.tests_passed, model.tests_failed, model.tests_ignored
        )));
    }
    lines.extend(cache(model));
    rimel::col(lines)
}

fn crop(block: Block, scroll: usize, width: u16, height: u16) -> Block {
    // Preserve styles while clipping both axes; browsing can reach every line
    // of a diagnostic even when the terminal is smaller than the upstream view.
    let mut lines: Vec<Vec<Block>> = vec![Vec::new(); usize::from(height)];
    for (x, y, text, style) in block.runs() {
        let y = usize::from(y);
        if y < scroll || y >= scroll + lines.len() || x >= width {
            continue;
        }
        let text_width = rimel::width_of(&text);
        let mut segment = rimel::text(text).style(style);
        if text_width > width - x {
            segment = segment.w(width - x);
        }
        lines[y - scroll].push(segment);
    }
    rimel::col(lines.into_iter().map(|line| {
        if line.is_empty() {
            rimel::text("")
        } else {
            rimel::row(line)
        }
    }))
}
