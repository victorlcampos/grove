//! Interface text in English and Brazilian Portuguese.

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Lang {
    En,
    Pt,
}

impl Lang {
    /// Portuguese when the locale asks for it, English otherwise.
    pub fn from_env() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        Self::from_locale(&locale)
    }

    fn from_locale(locale: &str) -> Self {
        if locale.to_ascii_lowercase().starts_with("pt") {
            Lang::Pt
        } else {
            Lang::En
        }
    }

    pub fn text(self) -> &'static Text {
        match self {
            Lang::En => &EN,
            Lang::Pt => &PT,
        }
    }
}

pub type Keys = &'static [(&'static str, &'static str)];

/// A text with a singular and a plural form; `{n}` stands for the count.
pub struct Plural {
    pub one: &'static str,
    pub many: &'static str,
}

impl Plural {
    pub fn of(&self, n: usize) -> String {
        let template = if n == 1 { self.one } else { self.many };
        template.replace("{n}", &n.to_string())
    }
}

pub struct Text {
    pub working: &'static str,
    pub blocked: &'static str,
    pub idle_sessions: &'static str,
    pub free_space: &'static str,
    pub disk: &'static str,
    pub measuring: &'static str,
    pub removing_count: &'static str,
    pub state_working: &'static str,
    pub state_blocked: &'static str,
    pub state_idle: &'static str,
    pub state_busy: &'static str,
    pub state_free: &'static str,
    pub state_missing: &'static str,
    pub state_removing: &'static str,
    pub main_tag: &'static str,
    pub locked_tag: &'static str,
    pub detached: &'static str,
    pub clean: &'static str,
    pub changed: Plural,
    pub untracked: &'static str,
    pub conflicts: &'static str,
    pub no_commits: &'static str,
    pub git_failed: &'static str,
    pub checking: &'static str,
    pub sessions_title: &'static str,
    pub no_sessions: &'static str,
    pub procs_title: &'static str,
    pub space_title: &'static str,
    pub files: &'static str,
    pub others: &'static str,
    pub not_measured: &'static str,
    pub background: &'static str,
    pub terminal: &'static str,
    pub last_used: &'static str,
    pub remove_title: &'static str,
    pub idle_title: &'static str,
    pub stops: Plural,
    pub will_be_lost: Plural,
    pub locked: &'static str,
    pub lock_stale: &'static str,
    pub warn_missing: &'static str,
    pub note_branch: &'static str,
    pub idle_summary: &'static str,
    pub idle_more: &'static str,
    pub idle_dirty: Plural,
    pub idle_command: &'static str,
    pub idle_none: &'static str,
    pub key_remove: &'static str,
    pub key_remove_all: &'static str,
    pub key_cancel: &'static str,
    pub removed: &'static str,
    pub removed_freed: &'static str,
    pub remove_failed: &'static str,
    pub idle_done: &'static str,
    pub idle_failed: &'static str,
    pub main_refused: &'static str,
    pub filter: &'static str,
    pub matches: &'static str,
    pub no_match: &'static str,
    pub sorts: [&'static str; 4],
    pub searching: &'static str,
    pub none_found: &'static str,
    pub none_hint: &'static str,
    pub help_title: &'static str,
    pub legend_title: &'static str,
    pub list_keys: Keys,
    pub filter_keys: Keys,
    pub details_keys: Keys,
    pub help_keys: Keys,
}

const EN: Text = Text {
    working: "working",
    blocked: "waiting for you",
    idle_sessions: "idle",
    free_space: "free",
    disk: "disk",
    measuring: "measuring",
    removing_count: "removing {n}",
    state_working: "working",
    state_blocked: "waiting for you",
    state_idle: "idle session",
    state_busy: "processes running",
    state_free: "free",
    state_missing: "folder is gone",
    state_removing: "removing…",
    main_tag: "main",
    locked_tag: "locked",
    detached: "detached",
    clean: "clean",
    changed: Plural {
        one: "{n} changed file",
        many: "{n} changed files",
    },
    untracked: "untracked",
    conflicts: "conflicts",
    no_commits: "no commits yet",
    git_failed: "git status failed",
    checking: "checking…",
    sessions_title: "SESSIONS",
    no_sessions: "No agent session here",
    procs_title: "PROCESSES",
    space_title: "SPACE",
    files: "files",
    others: "others",
    not_measured: "not measured yet",
    background: "background",
    terminal: "terminal",
    last_used: "last used",
    remove_title: "Remove worktree",
    idle_title: "Remove idle worktrees",
    stops: Plural {
        one: "Stops the process running in it:",
        many: "Stops the {n} processes running in it:",
    },
    will_be_lost: Plural {
        one: "1 changed or untracked file will be lost",
        many: "{n} changed or untracked files will be lost",
    },
    locked: "Locked: {reason}",
    lock_stale: "the session that locked it is gone",
    warn_missing: "The folder is already gone: this only clears git's record",
    note_branch: "Branch {branch} stays; delete it with git branch -d {branch}",
    idle_summary: "{n} worktrees with no agent session and no process · {size}",
    idle_more: "…and {n} more",
    idle_dirty: Plural {
        one: "1 of them has uncommitted files, which will be lost",
        many: "{n} of them have uncommitted files, which will be lost",
    },
    idle_command: "git worktree remove --force on each; the main worktrees stay",
    idle_none: "No idle worktree to remove",
    key_remove: "remove",
    key_remove_all: "remove all",
    key_cancel: "cancel",
    removed: "{name} removed",
    removed_freed: "{name} removed · {size} freed",
    remove_failed: "Could not remove {name}",
    idle_done: "{n} worktrees removed · {size} freed",
    idle_failed: "{done} removed, {failed} failed: {error}",
    main_refused: "The main worktree can't be removed",
    filter: "filter",
    matches: "{n} of {m}",
    no_match: "Nothing matches",
    sorts: ["activity", "size", "name", "oldest"],
    searching: "Looking for repositories…",
    none_found: "No worktrees found",
    none_hint: "Looked in {roots}. Pass other folders: grove ~/code ~/src",
    help_title: "Keys",
    legend_title: "Legend",
    list_keys: &[
        ("↑↓", "move"),
        ("⏎", "details"),
        ("d", "remove"),
        ("D", "remove idle"),
        ("/", "filter"),
        ("s", "sort"),
        ("r", "refresh"),
        ("?", "help"),
        ("q", "quit"),
    ],
    filter_keys: &[("⏎", "apply"), ("Esc", "clear")],
    details_keys: &[("Esc", "back"), ("d", "remove")],
    help_keys: &[
        ("↑ ↓  j k", "move"),
        ("PgUp PgDn", "page"),
        ("g  G", "first / last"),
        ("Enter", "show or hide the details"),
        (
            "d",
            "remove the worktree, stopping what runs in it (git worktree remove --force)",
        ),
        (
            "D",
            "remove every idle worktree: no agent session, no process",
        ),
        ("/", "filter by name, branch or session"),
        ("s", "sort: activity, size, name, oldest"),
        ("r", "refresh now"),
        ("R", "measure every worktree again"),
        ("?", "this help"),
        ("q", "quit"),
    ],
};

const PT: Text = Text {
    working: "trabalhando",
    blocked: "esperando você",
    idle_sessions: "ociosas",
    free_space: "livres",
    disk: "disco",
    measuring: "medindo",
    removing_count: "removendo {n}",
    state_working: "trabalhando",
    state_blocked: "esperando você",
    state_idle: "sessão ociosa",
    state_busy: "processos rodando",
    state_free: "livre",
    state_missing: "pasta sumiu",
    state_removing: "removendo…",
    main_tag: "principal",
    locked_tag: "travada",
    detached: "desanexada",
    clean: "limpa",
    changed: Plural {
        one: "{n} arquivo alterado",
        many: "{n} arquivos alterados",
    },
    untracked: "não rastreados",
    conflicts: "conflitos",
    no_commits: "ainda sem commits",
    git_failed: "o git status falhou",
    checking: "verificando…",
    sessions_title: "SESSÕES",
    no_sessions: "Nenhuma sessão de agente aqui",
    procs_title: "PROCESSOS",
    space_title: "ESPAÇO",
    files: "arquivos",
    others: "outros",
    not_measured: "ainda não medida",
    background: "segundo plano",
    terminal: "terminal",
    last_used: "último uso",
    remove_title: "Remover worktree",
    idle_title: "Remover worktrees inativas",
    stops: Plural {
        one: "Encerra o processo que roda nela:",
        many: "Encerra os {n} processos que rodam nela:",
    },
    will_be_lost: Plural {
        one: "1 arquivo alterado ou não rastreado vai se perder",
        many: "{n} arquivos alterados ou não rastreados vão se perder",
    },
    locked: "Travada: {reason}",
    lock_stale: "a sessão que travou já terminou",
    warn_missing: "A pasta já não existe: isso só limpa o registro do git",
    note_branch: "A branch {branch} continua; apague com git branch -d {branch}",
    idle_summary: "{n} worktrees sem sessão de agente nem processo · {size}",
    idle_more: "…e mais {n}",
    idle_dirty: Plural {
        one: "1 delas tem alterações não commitadas, que vão se perder",
        many: "{n} delas têm alterações não commitadas, que vão se perder",
    },
    idle_command: "git worktree remove --force em cada uma; as principais ficam",
    idle_none: "Nenhuma worktree inativa para remover",
    key_remove: "remover",
    key_remove_all: "remover todas",
    key_cancel: "cancelar",
    removed: "{name} removida",
    removed_freed: "{name} removida · {size} liberados",
    remove_failed: "Não deu para remover {name}",
    idle_done: "{n} worktrees removidas · {size} liberados",
    idle_failed: "{done} removidas, {failed} falharam: {error}",
    main_refused: "A worktree principal não pode ser removida",
    filter: "filtro",
    matches: "{n} de {m}",
    no_match: "Nada encontrado",
    sorts: ["atividade", "tamanho", "nome", "mais antigas"],
    searching: "Procurando repositórios…",
    none_found: "Nenhuma worktree encontrada",
    none_hint: "Procurei em {roots}. Passe outras pastas: grove ~/code ~/src",
    help_title: "Teclas",
    legend_title: "Legenda",
    list_keys: &[
        ("↑↓", "mover"),
        ("⏎", "detalhes"),
        ("d", "remover"),
        ("D", "remover inativas"),
        ("/", "filtrar"),
        ("s", "ordem"),
        ("r", "atualizar"),
        ("?", "ajuda"),
        ("q", "sair"),
    ],
    filter_keys: &[("⏎", "aplicar"), ("Esc", "limpar")],
    details_keys: &[("Esc", "voltar"), ("d", "remover")],
    help_keys: &[
        ("↑ ↓  j k", "mover"),
        ("PgUp PgDn", "página"),
        ("g  G", "primeira / última"),
        ("Enter", "mostrar ou esconder os detalhes"),
        (
            "d",
            "remover a worktree, encerrando o que roda nela (git worktree remove --force)",
        ),
        (
            "D",
            "remover todas as inativas: sem sessão de agente nem processo",
        ),
        ("/", "filtrar por nome, branch ou sessão"),
        ("s", "ordem: atividade, tamanho, nome, mais antigas"),
        ("r", "atualizar agora"),
        ("R", "medir todas de novo"),
        ("?", "esta ajuda"),
        ("q", "sair"),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_portuguese_from_the_locale() {
        assert_eq!(Lang::from_locale("pt_BR.UTF-8"), Lang::Pt);
        assert_eq!(Lang::from_locale("PT_pt"), Lang::Pt);
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Lang::En);
        assert_eq!(Lang::from_locale(""), Lang::En);
    }

    #[test]
    fn plurals_pick_the_form_by_count() {
        assert_eq!(PT.stops.of(1), "Encerra o processo que roda nela:");
        assert_eq!(PT.stops.of(3), "Encerra os 3 processos que rodam nela:");
        assert_eq!(EN.changed.of(0), "0 changed files");
    }

    #[test]
    fn both_languages_have_the_same_keys() {
        for (en, pt) in [
            (EN.list_keys, PT.list_keys),
            (EN.help_keys, PT.help_keys),
            (EN.filter_keys, PT.filter_keys),
            (EN.details_keys, PT.details_keys),
        ] {
            let keys = |list: Keys| list.iter().map(|(key, _)| *key).collect::<Vec<_>>();
            assert_eq!(keys(en), keys(pt));
        }
    }
}
