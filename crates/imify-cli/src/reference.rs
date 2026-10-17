//! The CLI reference in `docs/reference/cli.md` and the man pages in `docs/man/`, generated
//! from the command tree and compared with the committed files.
//!
//! `IMIFY_UPDATE_REFERENCE=1 cargo test -p imify-cli reference` rewrites the committed files.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{ColorChoice, Command};
use clap_mangen::Man;

/// The Markdown reference, relative to the repository root.
const MARKDOWN: &str = "docs/reference/cli.md";
/// The man page directory, relative to the repository root.
const MAN_DIR: &str = "docs/man";

/// Rewrites the committed reference and man pages.
const UPDATE_COMMAND: &str = "IMIFY_UPDATE_REFERENCE=1 cargo test -p imify-cli reference";

/// The generated reference and man pages.
struct Reference {
    markdown: String,
    man_pages: BTreeMap<String, String>,
}

impl Reference {
    fn generate(root: &Command) -> io::Result<Self> {
        Ok(Self {
            markdown: render_markdown(root),
            man_pages: render_man_pages(root)?,
        })
    }

    /// Writes the reference and the man pages under `repo` and removes man pages of commands
    /// that no longer exist.
    fn write_to(&self, repo: &Path) -> io::Result<()> {
        let markdown = repo.join(MARKDOWN);
        if let Some(parent) = markdown.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(markdown, &self.markdown)?;

        let man_dir = repo.join(MAN_DIR);
        fs::create_dir_all(&man_dir)?;
        for name in committed_man_pages(&man_dir)? {
            if !self.man_pages.contains_key(&name) {
                fs::remove_file(man_dir.join(name))?;
            }
        }
        for (name, page) in &self.man_pages {
            fs::write(man_dir.join(name), page)?;
        }
        Ok(())
    }

    /// Whether the reference under `repo` differs from the generated one or is missing.
    fn markdown_mismatch(&self, repo: &Path) -> io::Result<Option<Mismatch>> {
        compare(repo, Path::new(MARKDOWN), &self.markdown)
    }

    /// The man pages under `repo` that differ from the generated ones, are missing or belong to
    /// no command.
    fn man_page_mismatches(&self, repo: &Path) -> io::Result<Vec<Mismatch>> {
        let mut mismatches = Vec::new();
        for (name, page) in &self.man_pages {
            mismatches.extend(compare(repo, &Path::new(MAN_DIR).join(name), page)?);
        }
        for name in committed_man_pages(&repo.join(MAN_DIR))? {
            if !self.man_pages.contains_key(&name) {
                mismatches.push(Mismatch::Stale(Path::new(MAN_DIR).join(name)));
            }
        }
        Ok(mismatches)
    }
}

/// Compares the file at `path`, relative to `repo`, with `generated`.
fn compare(repo: &Path, path: &Path, generated: &str) -> io::Result<Option<Mismatch>> {
    match fs::read_to_string(repo.join(path)) {
        Ok(committed) if committed == generated => Ok(None),
        Ok(_) => Ok(Some(Mismatch::Differs(path.to_owned()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Some(Mismatch::Missing(path.to_owned())))
        }
        Err(error) => Err(error),
    }
}

/// The file names of the man pages in `man_dir`, sorted; none when the directory is missing.
fn committed_man_pages(man_dir: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(man_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".1") {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// A committed file that does not match the generated one. Holds the path relative to the
/// repository root.
#[derive(Debug, PartialEq, Eq)]
enum Mismatch {
    Differs(PathBuf),
    Missing(PathBuf),
    Stale(PathBuf),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Differs(path) => write!(f, "{} differs from the generated file", path.display()),
            Self::Missing(path) => write!(f, "{} is missing", path.display()),
            Self::Stale(path) => write!(f, "{} belongs to no command", path.display()),
        }
    }
}

/// Renders the Markdown reference: one section per command with its `--help` text.
fn render_markdown(root: &Command) -> String {
    let sections: Vec<String> = documented_commands(root)
        .into_iter()
        .map(|(path, mut command)| {
            let help = command.render_long_help().to_string();
            format!("\n## {path}\n\n```text\n{}\n```\n", help.trim_end())
        })
        .collect();
    format!("# CLI reference\n{}", sections.concat())
}

/// Renders one man page per documented command, keyed by file name such as `imify-auth-login.1`.
fn render_man_pages(root: &Command) -> io::Result<BTreeMap<String, String>> {
    let source = root.get_name().to_owned();
    let root = root.clone().disable_help_subcommand(true);
    documented_commands(&root)
        .into_iter()
        .map(|(_, command)| {
            let title = command
                .get_display_name()
                .unwrap_or_else(|| command.get_name())
                .to_owned();
            Ok((
                format!("{title}.1"),
                render_man_page(command, &title, &source)?,
            ))
        })
        .collect()
}

/// The string definition `roff` writes at the start of every rendered fragment.
const ROFF_PREAMBLE: &str = ".ie \\n(.g .ds Aq \\(aq\n.el .ds Aq '\n";

/// Renders the man page of `command` in section 1. The header has an empty date and `source`
/// without a version; the page has no VERSION section.
fn render_man_page(command: Command, title: &str, source: &str) -> io::Result<String> {
    type Section = fn(&Man, &mut dyn io::Write) -> io::Result<()>;

    let mut sections: Vec<Section> = vec![
        Man::render_name_section,
        Man::render_synopsis_section,
        Man::render_description_section,
        Man::render_options_section,
    ];
    if command.has_subcommands() {
        sections.push(Man::render_subcommands_section);
    }
    if command.get_after_long_help().is_some() || command.get_after_help().is_some() {
        sections.push(Man::render_extra_section);
    }

    let man = Man::new(command);
    let mut page = format!("{ROFF_PREAMBLE}.TH {title} 1 \"\" \"{source}\" \"\"\n");
    for render in sections {
        let mut fragment = Vec::new();
        render(&man, &mut fragment)?;
        let fragment = String::from_utf8(fragment)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let body = fragment.strip_prefix(ROFF_PREAMBLE).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "roff fragment without the preamble",
            )
        })?;
        page.push_str(body);
    }
    Ok(page)
}

/// Every non-hidden command except `help`, depth first in declaration order, with its path
/// such as `imify auth login`. The commands are built, so their help shows the full usage.
fn documented_commands(root: &Command) -> Vec<(String, Command)> {
    fn visit(command: &Command, path: &str, commands: &mut Vec<(String, Command)>) {
        commands.push((path.to_owned(), command.clone()));
        for subcommand in command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set() && subcommand.get_name() != "help")
        {
            visit(
                subcommand,
                &format!("{path} {}", subcommand.get_name()),
                commands,
            );
        }
    }

    let mut root = root.clone().color(ColorChoice::Never);
    root.build();
    let mut commands = Vec::new();
    visit(&root, root.get_name(), &mut commands);
    commands
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use clap::CommandFactory as _;

    use super::*;
    use crate::cli::Cli;

    /// The generated reference for the current command tree. With `IMIFY_UPDATE_REFERENCE=1`
    /// it is first written to the repository.
    static CURRENT: LazyLock<Reference> = LazyLock::new(|| {
        let reference = Reference::generate(&Cli::command()).unwrap();
        if std::env::var_os("IMIFY_UPDATE_REFERENCE").is_some_and(|value| value == "1") {
            reference.write_to(&repo_root()).unwrap();
        }
        reference
    });

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn assert_no_mismatches(mismatches: &[Mismatch]) {
        let report: Vec<String> = mismatches.iter().map(ToString::to_string).collect();
        assert!(
            mismatches.is_empty(),
            "{}\nRegenerate with: {UPDATE_COMMAND}",
            report.join("\n")
        );
    }

    #[test]
    fn committed_cli_reference_matches_the_command_tree() {
        let mismatch = CURRENT.markdown_mismatch(&repo_root()).unwrap();

        assert_no_mismatches(mismatch.as_slice());
    }

    #[test]
    fn committed_man_pages_match_the_command_tree() {
        let mismatches = CURRENT.man_page_mismatches(&repo_root()).unwrap();

        assert_no_mismatches(&mismatches);
    }

    #[test]
    fn cli_reference_and_man_pages_omit_the_package_version() {
        let version = env!("CARGO_PKG_VERSION");

        assert!(!CURRENT.markdown.contains(version));
        for (name, page) in &CURRENT.man_pages {
            assert!(!page.contains(version), "{name} contains the version");
        }
    }

    fn sample() -> Command {
        Command::new("tool")
            .version("9.8.7")
            .about("Sample tool")
            .subcommand(
                Command::new("thing")
                    .about("Work with things")
                    .subcommand(Command::new("ls").about("List things"))
                    .subcommand(Command::new("secret").hide(true)),
            )
    }

    #[test]
    fn markdown_has_one_section_per_visible_command_with_its_help() {
        let markdown = render_markdown(&sample());

        assert_eq!(
            markdown,
            "# CLI reference

## tool

```text
Sample tool

Usage: tool [COMMAND]

Commands:
  thing  Work with things
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help

  -V, --version
          Print version
```

## tool thing

```text
Work with things

Usage: tool thing [COMMAND]

Commands:
  ls    List things
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## tool thing ls

```text
List things

Usage: tool thing ls

Options:
  -h, --help
          Print help
```
"
        );
    }

    #[test]
    fn man_pages_cover_every_visible_command() {
        let pages = render_man_pages(&sample()).unwrap();

        let names: Vec<&str> = pages.keys().map(String::as_str).collect();
        assert_eq!(names, ["tool-thing-ls.1", "tool-thing.1", "tool.1"]);
    }

    #[test]
    fn man_page_header_has_section_1_and_no_date_or_version() {
        let pages = render_man_pages(&sample()).unwrap();

        let header = pages["tool-thing.1"]
            .lines()
            .find(|line| line.starts_with(".TH"));
        assert_eq!(header, Some(r#".TH tool-thing 1 "" "tool" """#));
    }

    #[test]
    fn reference_and_man_pages_omit_the_version() {
        let markdown = render_markdown(&sample());
        let pages = render_man_pages(&sample()).unwrap();

        assert!(!markdown.contains("9.8.7"));
        for (name, page) in pages {
            assert!(!page.contains("9.8.7"), "{name} contains the version");
            assert!(
                !page.contains(".SH VERSION"),
                "{name} has a VERSION section"
            );
        }
    }

    fn without_ls() -> Command {
        Command::new("tool")
            .about("Sample tool")
            .subcommand(Command::new("thing").about("Work with things"))
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writing_creates_the_reference_and_one_page_per_command() {
        let repo = tempfile::tempdir().unwrap();

        Reference::generate(&sample())
            .unwrap()
            .write_to(repo.path())
            .unwrap();

        let markdown = std::fs::read_to_string(repo.path().join(MARKDOWN)).unwrap();
        assert!(markdown.starts_with("# CLI reference\n"));
        assert_eq!(
            files_in(&repo.path().join(MAN_DIR)),
            ["tool-thing-ls.1", "tool-thing.1", "tool.1"]
        );
    }

    #[test]
    fn writing_removes_the_page_of_a_removed_command() {
        let repo = tempfile::tempdir().unwrap();
        Reference::generate(&sample())
            .unwrap()
            .write_to(repo.path())
            .unwrap();

        Reference::generate(&without_ls())
            .unwrap()
            .write_to(repo.path())
            .unwrap();

        assert_eq!(
            files_in(&repo.path().join(MAN_DIR)),
            ["tool-thing.1", "tool.1"]
        );
    }

    #[test]
    fn page_without_a_command_is_a_mismatch() {
        let repo = tempfile::tempdir().unwrap();
        let reference = Reference::generate(&sample()).unwrap();
        reference.write_to(repo.path()).unwrap();
        std::fs::write(repo.path().join(MAN_DIR).join("tool-gone.1"), "old").unwrap();

        let mismatches = reference.man_page_mismatches(repo.path()).unwrap();

        assert_eq!(
            mismatches,
            [Mismatch::Stale(Path::new(MAN_DIR).join("tool-gone.1"))]
        );
    }

    #[test]
    fn changed_and_missing_pages_are_mismatches() {
        let repo = tempfile::tempdir().unwrap();
        Reference::generate(&without_ls())
            .unwrap()
            .write_to(repo.path())
            .unwrap();
        std::fs::write(repo.path().join(MAN_DIR).join("tool.1"), "edited").unwrap();

        let mismatches = Reference::generate(&sample())
            .unwrap()
            .man_page_mismatches(repo.path())
            .unwrap();

        assert_eq!(
            mismatches,
            [
                Mismatch::Missing(Path::new(MAN_DIR).join("tool-thing-ls.1")),
                Mismatch::Differs(Path::new(MAN_DIR).join("tool-thing.1")),
                Mismatch::Differs(Path::new(MAN_DIR).join("tool.1")),
            ]
        );
    }
}
