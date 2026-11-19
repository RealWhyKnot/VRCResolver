use super::blocks::Item;
use super::renderer::Renderer;
use super::text::normalize_verb;
use crate::maintenance::update;

pub const MAX_SUGGESTION_SCORE: usize = 3;

pub struct Context<'a> {
    pub renderer: &'a Renderer,
    pub commands: &'a Registry,
}

pub type Handler = fn(&Context<'_>, &str);
#[derive(Clone, Copy)]
pub struct CompletionInput<'a> {
    pub slash: bool,
    pub tokens: &'a [&'a str],
    pub ends_with_space: bool,
}

pub type Completer = fn(&Registry, CompletionInput<'_>) -> Completion;

pub struct Command {
    pub name: String,
    pub description: String,
    handler: Handler,
    completer: Option<Completer>,
}

impl Command {
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
        r.add("help", "Show commands or get help with one.", help);
        r.add("status", "Show current video activity.", |ctx, _| {
            ctx.renderer.render_status()
        });
        r.add(
            "repair",
            "Ask Windows again for permission to play videos in public worlds.",
            |ctx, _| ctx.renderer.repair(),
        );
        r.add("update", "Check for and install updates.", update_command);
        r.add("clear", "Clear the console.", |ctx, _| {
            ctx.renderer.clear_screen()
        });
        r.set_completer("help", complete_help);
        r.set_completer("update", complete_update);
        r
    }

    pub fn add(&mut self, name: &str, description: &str, handler: Handler) {
        let name = normalize_verb(name);
        assert!(!name.is_empty(), "command name is required");
        self.commands.push(Command {
            name,
            description: description.to_owned(),
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
            .find(|c| c.name == verb)
            .unwrap_or_else(|| panic!("unknown command: {name}"));
        command.completer = Some(completer);
    }

    pub fn all(&self) -> &[Command] {
        &self.commands
    }

    pub fn get(&self, verb: &str) -> Option<&Command> {
        let verb = normalize_verb(verb);
        self.commands.iter().rev().find(|c| c.name == verb)
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
            Some(c) => c.completer.map_or_else(Completion::default, |f| {
                f(
                    self,
                    CompletionInput {
                        slash,
                        tokens: &tokens,
                        ends_with_space,
                    },
                )
            }),
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
        if candidate.chars().count() <= input.chars().count() || !starts_with_ci(&candidate, input)
        {
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
                let best = match_score(&verb, &c.name);
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
            .filter(|c| starts_with_ci(&c.name, &prefix))
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
        ctx.renderer
            .info("To uninstall, run vrcresolver.Uninstaller.exe from this folder.");
    } else if let Some(target) = ctx.commands.get(&wanted) {
        ctx.renderer.render_command_help(target);
        if target.name == "update" {
            ctx.renderer.render_update_help(&UPDATE_OPTIONS);
        }
    } else {
        ctx.renderer
            .render_unknown_command(&wanted, &ctx.commands.nearest(&wanted, 3));
    }
}

fn complete_help(registry: &Registry, input: CompletionInput<'_>) -> Completion {
    let CompletionInput {
        slash,
        tokens,
        ends_with_space,
    } = input;
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

const UPDATE_OPTIONS: [(&str, &str); 2] = [
    ("beta", "Check for updates and include beta releases."),
    ("stable", "Check for stable updates only."),
];

fn update_command(ctx: &Context<'_>, args: &str) {
    let requested = match args.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "beta" => Some(true),
        "stable" => Some(false),
        _ => {
            ctx.renderer
                .warn("Type update, update beta or update stable.");
            return;
        }
    };
    let store = ctx.renderer.settings();
    if let Some(include) = requested
        && store.set_include_prereleases(include).is_err()
    {
        ctx.renderer.error("Couldn't save your update choice.");
        return;
    }
    let channel = if store.include_prereleases() {
        "beta"
    } else {
        "stable"
    };
    match ctx.renderer.start_update() {
        update::UpdateStart::UpdaterMissing => ctx.renderer.error(&format!(
            "Couldn't check for {channel} updates because the updater is missing."
        )),
        update::UpdateStart::Failed => ctx
            .renderer
            .warn(&format!("Couldn't start the check for {channel} updates.")),
        update::UpdateStart::Started => ctx
            .renderer
            .info(&format!("Checking for {channel} updates.")),
    }
}

fn complete_update(_registry: &Registry, input: CompletionInput<'_>) -> Completion {
    let prefix = match (input.tokens, input.ends_with_space) {
        ([_], true) => "",
        ([_, prefix], false) => prefix,
        _ => return Completion::default(),
    };
    let items: Vec<_> = UPDATE_OPTIONS
        .iter()
        .filter(|(name, _)| starts_with_ci(name, prefix))
        .map(|(name, description)| Item::new(*name, *description))
        .collect();
    if let [item] = items.as_slice() {
        let slash = if input.slash { "/" } else { "" };
        return Completion::replace(format!("{slash}update {}", item.text));
    }
    Completion::list(items)
}
