//! `vendor search` - find programs in the registry.

use anyhow::Result;
use vendorfiles_core::{registry, ui};

/// `vendor search [terms]` / `vendor add --list` - the programs `install` knows by name.
///
/// One row per program, best match first. A program with no build for this machine stays in
/// the list, marked, so a search never seems to come up short for no reason.
pub(super) async fn search(query: &str, refresh: bool) -> Result<()> {
    const HEADERS: [&str; 3] = ["NAME", "ALIASES", "DESCRIPTION"];

    let registry = registry::load(refresh).await?;
    let found = registry::search::search(&registry, query);
    if found.is_empty() {
        ui::info(format!("no program in the registry matches '{query}'"));
        return Ok(());
    }

    let host = registry::resolve::host();
    let rows: Vec<[String; 3]> = found
        .iter()
        .map(|(name, program)| {
            let aliases = if program.aliases.is_empty() {
                "-".to_owned()
            } else {
                program.aliases.join(", ")
            };
            let description = program.description.as_deref().unwrap_or_default();
            let description = if program.covers(&host) {
                description.to_owned()
            } else {
                format!("{description} (no {host} build)")
            };
            [(*name).to_owned(), aliases, description]
        })
        .collect();

    // Character counts, not bytes: a non-ASCII name would otherwise pad short.
    let widths: [usize; 2] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain(std::iter::once(HEADERS[column].chars().count()))
            .max()
            .unwrap_or(0)
    });

    // The name pads by hand, since its link escapes would count toward a `{:<width$}`.
    let line = |name: &str, url: Option<&str>, cells: [&str; 2]| {
        let padding = " ".repeat(widths[0] - name.chars().count());
        let name = url.map_or_else(|| name.to_owned(), |url| ui::link(name, url));
        format!(
            "{name}{padding}  {:<aliases$}  {}",
            cells[0],
            cells[1],
            aliases = widths[1]
        )
    };

    vendorfiles_core::progress::print_out(&ui::cyan(&line(
        HEADERS[0],
        None,
        [HEADERS[1], HEADERS[2]],
    )));
    let rule = ui::rule();
    vendorfiles_core::progress::print_out(&rule);
    for (row, (_, program)) in rows.iter().zip(&found) {
        vendorfiles_core::progress::print_out(&line(
            &row[0],
            Some(&program.repository),
            [&row[1], &row[2]],
        ));
        vendorfiles_core::progress::print_out(&rule);
    }
    Ok(())
}
