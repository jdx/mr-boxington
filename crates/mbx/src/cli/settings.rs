use crate::config::{check_global_file, config_file_path, resolve_settings, settings_registry};
use eyre::{Context, Result, bail, eyre};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use toml_edit::{DocumentMut, Item, TableLike};
use usage_config::{PropMeta, Resolved, Scope, Ty, Value};

#[derive(usage::Args)]
pub(super) struct SettingsArgs {
    #[usage(subcommand)]
    pub(super) command: SettingsCommands,
}

#[derive(usage::Subcommands)]
pub(super) enum SettingsCommands {
    /// List settings and their current values.
    ///
    /// Values come from the environment, the global configuration file, and
    /// defaults. Workspace `.mbx.toml` settings are not included.
    Ls(LsArgs),
    /// Print the current value of one setting.
    ///
    /// The value comes from the environment, the global configuration file, or
    /// the default. Workspace `.mbx.toml` settings are not included. A setting
    /// with no value prints nothing.
    Get(KeyArgs),
    /// Write a setting to the global configuration file.
    ///
    /// The value must match the setting's type and allowed values, and the
    /// file must still load with it, or nothing is written. List settings take
    /// comma-separated items, such as `mbx settings set target.keep ~/src,/work`.
    /// Comments and formatting elsewhere in the file are kept. An environment
    /// variable for the same setting still takes precedence.
    Set(SetArgs),
    /// Remove a setting from the global configuration file, so it falls back to
    /// its default.
    Unset(KeyArgs),
}

#[derive(usage::Args)]
pub(super) struct LsArgs {
    /// Only list this setting, or the settings under this group, such as `gc`.
    key: Option<String>,
}

#[derive(usage::Args)]
pub(super) struct KeyArgs {
    /// Setting name, such as `gc.max_size`.
    key: String,
}

#[derive(usage::Args)]
pub(super) struct SetArgs {
    /// Setting name, such as `gc.max_size`.
    key: String,
    /// New value.
    #[usage(allow_negative_numbers)]
    value: String,
}

pub(super) fn run(args: SettingsArgs) -> Result<ExitCode> {
    match args.command {
        SettingsCommands::Ls(args) => list(args.key.as_deref()),
        SettingsCommands::Get(args) => get(&args.key),
        SettingsCommands::Set(args) => set(&args.key, &args.value),
        SettingsCommands::Unset(args) => unset(&args.key),
    }
    .map(|()| ExitCode::SUCCESS)
}

fn list(filter: Option<&str>) -> Result<()> {
    let resolved = resolved()?;
    let registry = resolved.registry();
    let mut ids = registry
        .ids()
        .filter(|id| {
            let meta = registry.get(*id);
            !meta.hide
                && meta.renamed_to.is_none()
                && filter.is_none_or(|filter| in_group(meta.key, filter))
        })
        .collect::<Vec<_>>();
    if let Some(filter) = filter
        && ids.is_empty()
    {
        bail!("unknown setting: {filter}");
    }
    ids.sort_by_key(|id| registry.get(*id).key);
    for id in ids {
        let meta = registry.get(id);
        match (resolved.get(id), meta.default_note) {
            (Some(value), _) => println!("{} = {}", meta.key, toml_value(value)),
            (None, Some(note)) => println!("{} is unset; default: {note}", meta.key),
            (None, None) => println!("{} is unset", meta.key),
        }
    }
    Ok(())
}

fn get(key: &str) -> Result<()> {
    let meta = setting(key)?;
    let resolved = resolved()?;
    match resolved.get_key(meta.key) {
        Some(Value::String(text)) => println!("{text}"),
        Some(value @ (Value::List(_) | Value::Map(_))) => println!("{}", toml_value(value)),
        Some(value) => println!("{value}"),
        None => {}
    }
    Ok(())
}

fn set(key: &str, raw: &str) -> Result<()> {
    let meta = setting(key)?;
    let path = file_path()?;
    let mut document = read(&path)?;
    set_value(&mut document, meta, raw, &path)?;
    save(&path, &document)?;
    note_environment(meta);
    Ok(())
}

fn unset(key: &str) -> Result<()> {
    let meta = setting(key)?;
    let path = file_path()?;
    if !path.try_exists()? {
        return Ok(());
    }
    let mut document = read(&path)?;
    if remove_value(&mut document, meta, &path)? {
        save(&path, &document)?;
        note_environment(meta);
    }
    Ok(())
}

/// Write `raw` into `document` as the value of `meta`.
///
/// `path` only names the file in errors.
pub(super) fn set_value(
    document: &mut DocumentMut,
    meta: &PropMeta,
    raw: &str,
    path: &Path,
) -> Result<()> {
    if meta.scope == Scope::Env {
        match meta.envs.first() {
            Some(env) => bail!(
                "{} can only be set in the environment, with {env}",
                meta.key
            ),
            None => bail!("{} can only be set in the environment", meta.key),
        }
    }
    if matches!(meta.ty.inner(), Ty::Map(_) | Ty::Object) {
        bail!(
            "{} is a table; edit {} to change it",
            meta.key,
            path.display()
        );
    }
    let mut value = toml_value(&parse(meta, raw)?);
    let (table, name, inline) =
        parent_table(document, meta.key, true, path)?.expect("missing tables are created");
    match table.get_mut(name) {
        // Replaced in place, so a comment above the key and one after the old
        // value both stay where they were.
        Some(Item::Value(previous)) => {
            *value.decor_mut() = previous.decor().clone();
            *previous = value;
        }
        _ => {
            table.insert(name, Item::Value(value));
            // An inline table keeps the spacing its last value had before the
            // closing brace, which would now sit before a comma.
            if inline {
                table.fmt();
            }
        }
    }
    Ok(())
}

/// Remove `meta` from `document`, reporting whether it was there.
pub(super) fn remove_value(
    document: &mut DocumentMut,
    meta: &PropMeta,
    path: &Path,
) -> Result<bool> {
    let Some((table, name, _)) = parent_table(document, meta.key, false, path)? else {
        return Ok(false);
    };
    if table.remove(name).is_none() {
        return Ok(false);
    }
    remove_empty_group(document, meta.key);
    Ok(true)
}

/// The setting `key` names, following a rename to its current name.
pub(super) fn setting(key: &str) -> Result<&'static PropMeta> {
    let registry = settings_registry();
    if let Some(found) = registry.lookup(key) {
        return Ok(registry.get(found.id));
    }
    if registry
        .props
        .iter()
        .any(|meta| !meta.hide && in_group(meta.key, key))
    {
        bail!("{key} is a group of settings; `mbx settings ls {key}` lists them");
    }
    bail!("unknown setting: {key}; `mbx settings ls` lists them")
}

fn in_group(key: &str, group: &str) -> bool {
    key.strip_prefix(group)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

fn resolved() -> Result<Resolved> {
    let resolved = resolve_settings()?;
    for warning in &resolved.warnings {
        match &warning.origin {
            Some(origin) => log::warn!("{} ({})", warning.message, origin.describe()),
            None => log::warn!("{}", warning.message),
        }
    }
    Ok(resolved)
}

/// Read `raw` the way the environment variable for this setting would be read.
fn parse(meta: &PropMeta, raw: &str) -> Result<Value> {
    let text = match meta.parse {
        Some(parser) => parser.split(raw),
        None => Value::String(raw.to_owned()),
    };
    let value = meta.ty.coerce(text).map_err(|error| {
        eyre!(
            "invalid {}: expected {}, found `{}`",
            meta.key,
            error.expected,
            error.found
        )
    })?;
    if let Some(refused) = meta.refuses(&value) {
        bail!(
            "invalid {}: `{refused}` is not one of {}",
            meta.key,
            meta.allowed()
        );
    }
    Ok(value)
}

fn toml_value(value: &Value) -> toml_edit::Value {
    match value {
        Value::Bool(value) => (*value).into(),
        Value::Int(value) => (*value).into(),
        Value::Float(value) => (*value).into(),
        Value::String(value) => value.as_str().into(),
        Value::List(items) => items
            .iter()
            .map(toml_value)
            .collect::<toml_edit::Array>()
            .into(),
        Value::Map(entries) => entries
            .iter()
            .map(|(key, value)| (key.as_str(), toml_value(value)))
            .collect::<toml_edit::InlineTable>()
            .into(),
    }
}

fn file_path() -> Result<PathBuf> {
    config_file_path()
        .ok_or_else(|| eyre!("the platform configuration directory could not be located"))
}

fn read(path: &Path) -> Result<DocumentMut> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
        }
    };
    contents
        .parse()
        .wrap_err_with(|| format!("failed to parse {}", path.display()))
}

/// The table holding `key`, the key's last segment, and whether that table is
/// inline, creating the tables on the way when `create` is set.
fn parent_table<'a>(
    document: &'a mut DocumentMut,
    key: &'a str,
    create: bool,
    path: &Path,
) -> Result<Option<(&'a mut dyn TableLike, &'a str, bool)>> {
    let (groups, name) = match key.rsplit_once('.') {
        Some((groups, name)) => (Some(groups), name),
        None => (None, key),
    };
    // A table added after existing content gets a blank line above its header.
    let spaced = !document.as_table().is_empty();
    let mut table: &mut dyn TableLike = document.as_table_mut();
    let mut inline = false;
    for group in groups.into_iter().flat_map(|groups| groups.split('.')) {
        if create && !table.contains_key(group) {
            let mut created = toml_edit::Table::new();
            if spaced {
                created.decor_mut().set_prefix("\n");
            }
            table.insert(group, Item::Table(created));
        }
        let Some(item) = table.get_mut(group) else {
            return Ok(None);
        };
        inline = item.is_inline_table();
        table = item
            .as_table_like_mut()
            .ok_or_else(|| eyre!("{group} in {} is not a table", path.display()))?;
    }
    Ok(Some((table, name, inline)))
}

/// Drop the table `key` was in once nothing is left in it, unless a comment
/// sits on its header.
fn remove_empty_group(document: &mut DocumentMut, key: &str) {
    let Some((group, _)) = key.split_once('.') else {
        return;
    };
    let Some(Item::Table(table)) = document.get(group) else {
        return;
    };
    let commented = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .is_some_and(|prefix| prefix.contains('#'));
    if table.is_empty() && !commented {
        document.remove(group);
    }
}

/// Write `document` to `path` once it loads as a configuration.
fn save(path: &Path, document: &DocumentMut) -> Result<()> {
    let contents = document.to_string();
    // The check reads through the file layer, which treats a missing file as
    // empty without looking at the text it is handed. An empty file is a valid
    // configuration, so it can stand in until the real one is written.
    let created = !path.try_exists()?;
    if created {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(path, "")
            .wrap_err_with(|| format!("failed to create {}", path.display()))?;
    }
    if let Err(error) = check_global_file(path, contents.clone()) {
        if created {
            let _ = std::fs::remove_file(path);
        }
        return Err(error.wrap_err(format!("{} was not changed", path.display())));
    }
    // Through any link, so a managed dotfile stays a link.
    let target = std::fs::canonicalize(path)
        .wrap_err_with(|| format!("failed to resolve {}", path.display()))?;
    crate::util::write_atomic(&target, contents.as_bytes())
}

fn note_environment(meta: &PropMeta) {
    for env in meta.envs {
        if std::env::var_os(env).is_some() {
            log::warn!("{env} is set and takes precedence over the configuration file");
        }
    }
}
