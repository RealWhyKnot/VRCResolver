use super::blocks::Item;
use super::renderer::Renderer;
use super::session::SessionStore;
use super::text::normalize_verb;
use crate::maintenance::update;
use crate::settings;

pub const MAX_SUGGESTION_SCORE: usize = 3;

pub struct Context<'a> {
    pub renderer: &'a Renderer,
    pub session: &'a SessionStore,
    pub commands: &'a Registry,
}

pub type Handler = fn(&Context<'_>, &str);
pub type Completer = fn(&Registry, bool, &[&str], bool) -> Completion;

pub struct Command {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    handler: Handler,
    completer: Option<Completer>,
}

impl Command {
    pub fn has_completer(&self) -> bool {
        self.completer.is_some()
    }

    pub fn run(&self, ctx: &Context<'_>, args: &str) {
        (self.handler)(ctx, args);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    pub replacement: String,
    pub suggestions: Vec<Item>,
}

impl Completion {
    fn replace(text: String) -> Completion {
        Completion {
            replacement: text,
            suggestions: Vec::new(),
        }
    }

    fn list(items: Vec<Item>) -> Completion {
        Completion {
            replacement: String::new(),
            suggestions: items,
        }
    }
}

#[derive(Default)]
pub struct Registry {
    commands: Vec<Command>,
}

fn starts_with_ci(value: &str, prefix: &str) -> bool {
    value.to_lowercase().starts_with(&prefix.to_lowercase())
}

impl Registry {
    pub fn with_defaults() -> Registry {
        let mut r = Registry::default();
        r.add(
            "help",
            "Show available terminal commands, or details for one of them.",
            help,
            &["?"],
        );
        r.add(
            "settings",
            "List or change persisted watchdog settings.",
            settings_command,
            &["config"],
        );
        r.add(
            "status",
            "Show what the proxy is doing right now.",
            |ctx, _| ctx.renderer.render_status(),
            &["stats", "activity", "dashboard"],
        );
        r.add(
            "history",
            "Show recent terminal commands.",
            |ctx, _| ctx.renderer.render_history(&ctx.session.recent_commands()),
            &[],
        );
        r.add(
            "permissions",
            "Show what this terminal is allowed to do.",
            |ctx, _| {
                ctx.renderer
                    .info("permissions: local watchdog commands only; shell execution is not exposed.");
            },
            &[],
        );
        r.add(
            "tools",
            "Show managed watchdog subsystems.",
            |ctx, _| ctx.renderer.render_tools(),
            &[],
        );
        r.add(
            "diagnostics",
            "Show useful support paths and session files.",
            |ctx, _| ctx.renderer.render_diagnostics(ctx.session),
            &["diag", "paths"],
        );
        r.add(
            "update",
            "Check for and install the latest VRCResolver release.",
            update_command,
            &["upgrade"],
        );
        r.add(
            "clear",
            "Clear the terminal view.",
            |ctx, _| ctx.renderer.clear_screen(),
            &["cls"],
        );
        r.add(
            "quit",
            "Shut down VRCResolver cleanly.",
            |ctx, _| {
                ctx.renderer.info("quit requested; shutting down.");
                ctx.renderer.request_shutdown();
            },
            &["exit"],
        );
        r.set_completer("settings", complete_settings);
        r.set_completer("help", complete_help);
        r
    }

    pub fn add(&mut self, name: &str, description: &str, handler: Handler, aliases: &[&str]) {
        let name = normalize_verb(name);
        assert!(!name.is_empty(), "command name is required");
        let aliases = aliases
            .iter()
            .map(|a| normalize_verb(a))
            .filter(|a| !a.is_empty())
            .collect();
        self.commands.push(Command {
            name,
            description: description.to_owned(),
            aliases,
            handler,
            completer: None,
        });
    }

    pub fn set_completer(&mut self, name: &str, completer: Completer) {
        let verb = normalize_verb(name);
        let command = self
            .commands
            .iter_mut()
            .rev()
            .find(|c| c.name == verb || c.aliases.contains(&verb))
            .unwrap_or_else(|| panic!("unknown command: {name}"));
        command.completer = Some(completer);
    }

    pub fn all(&self) -> &[Command] {
        &self.commands
    }

    pub fn get(&self, verb: &str) -> Option<&Command> {
        let verb = normalize_verb(verb);
        self.commands
            .iter()
            .rev()
            .find(|c| c.name == verb || c.aliases.contains(&verb))
    }

    pub fn complete(&self, input: &str) -> Completion {
        let trimmed = input.trim_start();
        let slash = trimmed.starts_with('/');
        let body = if slash { &trimmed[1..] } else { trimmed };
        let ends_with_space = body.chars().last().is_some_and(char::is_whitespace);
        let tokens: Vec<&str> = body.split([' ', '\t']).filter(|t| !t.is_empty()).collect();
        if !ends_with_space && tokens.len() <= 1 {
            return self.complete_command(slash, tokens.first().copied().unwrap_or(""));
        }
        let Some(first) = tokens.first() else {
            return self.complete_command(slash, "");
        };
        match self.get(first) {
            Some(c) => c
                .completer
                .map_or_else(Completion::default, |f| f(self, slash, &tokens, ends_with_space)),
            None => Completion::default(),
        }
    }

    pub fn suggest(&self, input: &str) -> String {
        if input.is_empty() || input.chars().last().is_some_and(char::is_whitespace) {
            return String::new();
        }
        let completion = self.complete(input);
        let mut candidate = completion.replacement;
        if candidate.is_empty() && completion.suggestions.len() == 1 {
            candidate.clone_from(&completion.suggestions[0].text);
        }
        if candidate.chars().count() <= input.chars().count() || !starts_with_ci(&candidate, input) {
            return String::new();
        }
        candidate.chars().skip(input.chars().count()).collect()
    }

    pub fn nearest(&self, verb: &str, max: usize) -> Vec<Item> {
        let verb = normalize_verb(verb);
        if verb.is_empty() || max == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(&Command, usize)> = self
            .commands
            .iter()
            .filter_map(|c| {
                let best = c
                    .aliases
                    .iter()
                    .map(|a| match_score(&verb, a))
                    .fold(match_score(&verb, &c.name), usize::min);
                (best <= MAX_SUGGESTION_SCORE).then_some((c, best))
            })
            .collect();
        scored.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()))
        });
        scored
            .into_iter()
            .take(max)
            .map(|(c, _)| Item::new(&c.name, &c.description))
            .collect()
    }

    fn complete_command(&self, slash: bool, token: &str) -> Completion {
        let prefix = normalize_verb(token);
        let mut matches: Vec<&Command> = self
            .commands
            .iter()
            .filter(|c| {
                starts_with_ci(&c.name, &prefix) || c.aliases.iter().any(|a| starts_with_ci(a, &prefix))
            })
            .collect();
        matches.sort_by_key(|c| c.name.to_lowercase());
        let lead = if slash { "/" } else { "" };
        if !prefix.is_empty() && matches.len() == 1 {
            return Completion::replace(format!("{lead}{} ", matches[0].name));
        }
        Completion::list(
            matches
                .iter()
                .map(|c| Item::new(format!("{lead}{}", c.name), &c.description))
                .collect(),
        )
    }
}

fn match_score(verb: &str, candidate: &str) -> usize {
    if starts_with_ci(candidate, verb) {
        0
    } else if is_subsequence(verb, candidate) {
        1
    } else {
        edit_distance(verb, candidate)
    }
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    let mut i = 0;
    for c in haystack.to_lowercase().chars() {
        if i < needle.len() && c == needle[i] {
            i += 1;
        }
    }
    i == needle.len()
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (current[j - 1] + 1)
                .min(previous[j] + 1)
                .min(previous[j - 1] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn help(ctx: &Context<'_>, args: &str) {
    let wanted = normalize_verb(args);
    if wanted.is_empty() {
        ctx.renderer.render_help(ctx.commands.all());
    } else if let Some(target) = ctx.commands.get(&wanted) {
        ctx.renderer.render_command_help(target);
    } else {
        ctx.renderer
            .render_unknown_command(&wanted, &ctx.commands.nearest(&wanted, 3));
    }
}

fn complete_help(registry: &Registry, slash: bool, tokens: &[&str], ends_with_space: bool) -> Completion {
    let prefix = if tokens.len() > 1 && !ends_with_space {
        normalize_verb(tokens[tokens.len() - 1])
    } else {
        String::new()
    };
    let lead = if slash { "/help " } else { "help " };
    let mut matches: Vec<&Command> = registry
        .all()
        .iter()
        .filter(|c| starts_with_ci(&c.name, &prefix))
        .collect();
    matches.sort_by_key(|c| c.name.to_lowercase());
    if matches.len() == 1 {
        return Completion::replace(format!("{lead}{}", matches[0].name));
    }
    Completion::list(
        matches
            .iter()
            .map(|c| Item::new(&c.name, &c.description))
            .collect(),
    )
}

fn update_command(ctx: &Context<'_>, _args: &str) {
    let started = ctx.renderer.start_update();
    let message = started.message();
    match started {
        update::UpdateStart::UpdaterMissing(_) => ctx.renderer.error(&message),
        update::UpdateStart::Failed => ctx.renderer.warn(&message),
        update::UpdateStart::Started => ctx.renderer.info(&message),
    }
}

fn settings_command(ctx: &Context<'_>, args: &str) {
    let parts: Vec<&str> = args.split(' ').map(str::trim).filter(|p| !p.is_empty()).collect();
    let Some(first) = parts.first() else {
        ctx.renderer.render_settings();
        return;
    };
    match first.to_lowercase().as_str() {
        "help" => ctx.renderer.render_settings_help(),
        "get" if parts.len() == 2 => render_one(ctx, parts[1]),
        "get" => ctx.renderer.warn("usage: settings get <name>"),
        "set" if parts.len() >= 3 => set_one(ctx, parts[1], &parts[2..].join(" ")),
        "set" => ctx.renderer.warn("usage: settings set <name> <value>"),
        "reset" if parts.len() == 2 => reset(ctx, parts[1]),
        "reset" => ctx.renderer.warn("usage: settings reset <name|all>"),
        _ if parts.len() >= 2 => set_one(ctx, parts[0], &parts[1..].join(" ")),
        _ => render_one(ctx, parts[0]),
    }
}

fn unknown_setting(ctx: &Context<'_>, key: &str) {
    ctx.renderer
        .warn(&format!("unknown setting: {key} (use settings to list names)"));
}

fn render_one(ctx: &Context<'_>, key: &str) {
    match settings::find(key) {
        Some(setting) => ctx.renderer.render_setting(setting),
        None => unknown_setting(ctx, key),
    }
}

fn set_one(ctx: &Context<'_>, key: &str, value: &str) {
    let Some(setting) = settings::find(key) else {
        unknown_setting(ctx, key);
        return;
    };
    let store = ctx.renderer.settings();
    let mut probe = store.snapshot();
    if let Err(e) = setting.try_set(&mut probe, value) {
        ctx.renderer.warn(&format!("invalid value for {key}: {e}"));
        return;
    }
    let next = store.update(|s| {
        let _ = setting.try_set(s, value);
    });
    let suffix = if setting.restart_required {
        " (takes effect next launch)"
    } else {
        ""
    };
    ctx.renderer
        .success(&format!("settings: {key} = {}{suffix}", setting.get(&next)));
}

fn reset(ctx: &Context<'_>, key: &str) {
    let store = ctx.renderer.settings();
    if key.eq_ignore_ascii_case("all") {
        store.update(|s| {
            for setting in &settings::ALL {
                setting.reset(s);
            }
        });
        ctx.renderer.success("settings: all values reset to defaults.");
        return;
    }
    let Some(setting) = settings::find(key) else {
        unknown_setting(ctx, key);
        return;
    };
    let next = store.update(|s| setting.reset(s));
    ctx.renderer
        .success(&format!("settings: {key} reset to {}", setting.get(&next)));
}

fn complete_settings(
    _registry: &Registry,
    slash: bool,
    tokens: &[&str],
    ends_with_space: bool,
) -> Completion {
    let mut args: Vec<&str> = tokens.iter().skip(1).copied().collect();
    if ends_with_space || args.is_empty() {
        args.push("");
    }
    let lead = if slash { "/settings" } else { "settings" };
    let current = args[args.len() - 1];
    if args.len() == 1 {
        let mut items = action_items(current);
        items.extend(setting_items(current, false));
        if items.len() == 1 {
            return Completion::replace(format!("{lead} {} ", items[0].text));
        }
        return Completion::list(items);
    }
    let action = args[0].to_lowercase();
    if action == "get" || action == "reset" {
        let items = setting_items(current, action == "reset");
        if items.len() == 1 {
            return Completion::replace(format!("{lead} {action} {} ", items[0].text));
        }
        return Completion::list(items);
    }
    if action == "set" {
        if args.len() == 2 {
            let items = setting_items(current, false);
            if items.len() == 1 {
                return Completion::replace(format!("{lead} set {} ", items[0].text));
            }
            return Completion::list(items);
        }
        return complete_value(&format!("{lead} set "), args[1], current);
    }
    complete_value(&format!("{lead} "), args[0], current)
}

fn complete_value(prefix: &str, name: &str, current: &str) -> Completion {
    let Some(setting) = settings::find(name) else {
        return Completion::default();
    };
    let items: Vec<Item> = setting
        .choices()
        .iter()
        .filter(|v| starts_with_ci(v, current))
        .map(|v| Item::new(*v, setting.description))
        .collect();
    if items.len() == 1 {
        return Completion::replace(format!("{prefix}{} {}", setting.key, items[0].text));
    }
    Completion::list(items)
}

fn action_items(prefix: &str) -> Vec<Item> {
    [
        ("get", "Show one setting."),
        ("set", "Change a setting."),
        ("reset", "Reset a setting."),
        ("help", "Show settings help."),
    ]
    .into_iter()
    .filter(|(t, _)| starts_with_ci(t, prefix))
    .map(|(t, d)| Item::new(t, d))
    .collect()
}

fn setting_items(prefix: &str, include_all: bool) -> Vec<Item> {
    let mut items = Vec::new();
    if include_all && starts_with_ci("all", prefix) {
        items.push(Item::new("all", "Reset every setting."));
    }
    for s in &settings::ALL {
        if starts_with_ci(s.key, prefix) || s.aliases.iter().any(|a| starts_with_ci(a, prefix)) {
            items.push(Item::new(s.key, s.description));
        }
    }
    items
}
