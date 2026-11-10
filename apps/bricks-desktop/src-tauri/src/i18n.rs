//! The interface language of the desktop backend: what it says to a person
//! (errors, labels) in the language chosen in the preferences. Every message
//! lives in the catalogue below; `tr!` picks one and fills its `{placeholders}`.
//! What goes to the model, logs and stored data are not translated.

use crate::contract::Language;
use std::sync::atomic::{AtomicBool, Ordering};

static FRENCH: AtomicBool = AtomicBool::new(false);

/// The language of the next messages; the preferences hold the choice.
pub fn set_language(language: Language) {
    FRENCH.store(language == Language::Fr, Ordering::Relaxed);
}

/// Whichever of two texts the engine gives in both languages.
pub fn pick<'a>(en: &'a str, fr: &'a str) -> &'a str {
    if FRENCH.load(Ordering::Relaxed) {
        fr
    } else {
        en
    }
}

/// A message in both languages.
#[derive(Debug, Clone, Copy)]
pub struct Message {
    pub en: &'static str,
    pub fr: &'static str,
}

/// The text of `message` in the current language, with `params` filled in.
pub fn render(message: Message, params: &[(&str, String)]) -> String {
    let template = if FRENCH.load(Ordering::Relaxed) {
        message.fr
    } else {
        message.en
    };
    // One pass: a value that holds `{e}` is not taken for a placeholder.
    let mut text = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        rest = &rest[open..];
        let filled = rest.find('}').and_then(|close| {
            let value = params.iter().find(|(name, _)| *name == &rest[1..close])?;
            Some((close, &value.1))
        });
        match filled {
            Some((close, value)) => {
                text.push_str(value);
                rest = &rest[close + 1..];
            }
            None => {
                text.push('{');
                rest = &rest[1..];
            }
        }
    }
    text.push_str(rest);
    text
}

/// `tr!(msg::NAME)` or `tr!(msg::NAME, path = shown, e = e)`.
macro_rules! tr {
    ($message:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::render($message, &[$((stringify!($name), $value.to_string())),*])
    };
}
pub(crate) use tr;

macro_rules! messages {
    ($($name:ident: $en:expr, $fr:expr;)*) => {
        /// The catalogue: every message the backend shows to a person.
        pub mod msg {
            use super::Message;
            $(pub const $name: Message = Message { en: $en, fr: $fr };)*
        }
    };
}

messages! {
    AGGRESSIVE: "aggressive", "agressive";
    ATTACHMENT: "attachment", "pièce jointe";
    ATTACHMENT_NOT_FOUND: "attachment not found", "pièce jointe introuvable";
    A_CONVERSATION_OF_THIS_PROJECT_IS: "a conversation of this project is running: cancel it or wait for it to finish", "une conversation de ce projet est en cours : annulez-la ou attendez sa fin";
    A_FOLDER_ISNT_COPIED_IT_STAYS: "a folder isn’t copied: it stays a reference", "un dossier n'est pas copié : il reste une référence";
    A_MODEL_ALREADY_EXISTS_IN_THIS: "a model “{id}” already exists in this provider", "un modèle « {id} » existe déjà dans ce fournisseur";
    A_PAGE_HOLDS_1_TO_50: "a page holds 1 to 50 entries", "une page compte de 1 à 50 entrées";
    A_PROVIDER_ALREADY_EXISTS: "a provider “{id}” already exists", "un fournisseur « {id} » existe déjà";
    A_RANGE_END_WITH_NO_START: "a range end with no start line", "une fin de plage sans ligne de début";
    A_RUN_IS_IN_PROGRESS_CANCEL: "a run is in progress: cancel it or wait for it to finish", "une exécution est en cours : annulez-la ou attendez sa fin";
    BINARY_FILE: "binary file", "fichier binaire";
    BINARY_FILE_OR_NON_UTF_8: "binary file or non-UTF-8 text", "fichier binaire ou texte non UTF-8";
    BROKEN_SYMBOLIC_LINK: "broken symbolic link", "lien symbolique cassé";
    CHANGED_TYPE_SINCE_THE_LISTING: "`{path}` changed type since the listing", "`{path}` a changé de type depuis la liste";
    CHANGE_REFUSED: "change refused: {e}", "modification refusée : {e}";
    COMMAND_LABELS: "command labels", "étiquettes des commandes";
    CONCURRENT_BACKGROUND_JOBS: "Concurrent background jobs", "Tâches de fond simultanées";
    CONCURRENT_SUB_AGENTS: "Concurrent sub-agents", "Sous-agents simultanés";
    CONFIGURATION_WARNINGS: "Configuration warnings", "Avertissements de configuration";
    CONVERSATIONS_WITHOUT_A_PROJECT_HAVE_NO: "conversations without a project have no folder to move", "les conversations sans projet n'ont pas de dossier à déplacer";
    CONVERSATION_ATTACHMENTS: "conversation attachments", "pièces jointes de la conversation";
    CONVERSATION_FULL_ITS_ATTACHMENTS_CANT_EXCEED: "conversation full: its attachments can’t exceed {detail} MB", "conversation pleine : ses pièces jointes ne peuvent pas dépasser {detail} Mo";
    COULD_NOT_FORWARD_THE_COMMAND: "could not forward the command: {e}", "commande impossible à transmettre : {e}";
    COULD_NOT_GET_THE_GIT_DIFF: "Could not get the Git diff", "Diff Git impossible";
    COULD_NOT_OPEN_A_TERMINAL: "Could not open a terminal", "Impossible d'ouvrir un terminal";
    COULD_NOT_READ_GITS_OUTPUT: "Could not read Git’s output ({e}).", "Lecture de la sortie Git impossible ({e}).";
    COULD_NOT_READ_THE_COMMITS_DIFF: "Could not read the commit’s diff", "Diff du commit illisible";
    COULD_NOT_READ_THE_COMMITS_FILE: "Could not read the commit’s file list", "Liste des fichiers du commit illisible";
    COULD_NOT_READ_THE_GIT_STATUS: "Could not read the Git status", "Lecture du statut Git impossible";
    COULD_NOT_READ_THE_STAGED_CHANGES: "Could not read the staged changes", "Lecture des changements indexés impossible";
    COULD_NOT_READ_THE_TERMINAL: "Could not read the terminal", "Impossible de lire le terminal";
    COULD_NOT_READ_THE_UNSTAGED_CHANGES: "Could not read the unstaged changes", "Lecture des changements non indexés impossible";
    COULD_NOT_RESIZE_THE_TERMINAL: "Could not resize the terminal", "Redimensionnement du terminal impossible";
    COULD_NOT_SHOW_IT_IN_THE: "could not show it in the Finder: {e}", "affichage dans le Finder impossible : {e}";
    COULD_NOT_START_THE_TERMINAL: "Could not start the terminal", "Impossible de démarrer le terminal";
    COULD_NOT_WAIT_FOR_GIT: "Could not wait for Git ({e}).", "Attente de Git impossible ({e}).";
    COULD_NOT_WRITE_TO_THE_TERMINAL: "Could not write to the terminal", "Impossible d'écrire dans le terminal";
    CUA_DRIVERS_MCP_SERVER_DIDNT_START: "Cua Driver’s MCP server didn’t start: {e}", "le serveur MCP de Cua Driver n’a pas démarré : {e}";
    CUA_DRIVER_CANT_BE_FOUND: "Cua Driver can’t be found", "Cua Driver est introuvable";
    CUA_DRIVER_COULD_NOT_BE_LAUNCHED: "Cua Driver could not be launched: {e}", "Cua Driver n’a pas pu être lancé : {e}";
    CUA_DRIVER_COULD_NOT_BE_LAUNCHED_2: "Cua Driver could not be launched: {detail}", "Cua Driver n’a pas pu être lancé : {detail}";
    CUA_DRIVER_DOESNT_ANSWER_AFTER_ITS: "Cua Driver doesn’t answer after its launch", "Cua Driver ne répond pas après son lancement";
    DELETING_THE_CONVERSATION: "deleting the conversation", "suppression de la conversation";
    DIAGRAM_TOO_LARGE_TO_SAVE_5: "diagram too large to save (5 MiB at most)", "diagramme trop volumineux pour être enregistré (5 Mio au maximum)";
    DOESNT_EXIST_IN_THE_PROJECT: "`{rel}` doesn’t exist in the project", "`{rel}` n'existe pas dans le projet";
    DRAFT_FULL_ITS_ATTACHMENTS_CANT_EXCEED: "draft full: its attachments can’t exceed {detail} MB", "brouillon plein : ses pièces jointes ne peuvent pas dépasser {detail} Mo";
    EXCEL_WORKBOOK: "Excel workbook", "classeur Excel";
    FILES_ANALYSED_BY_SEMANTIC_SEARCH: "Files analysed by semantic search", "Fichiers analysés par la recherche sémantique";
    FILE_DOESNT_EXIST: "file {origin} doesn’t exist", "le fichier {origin} n'existe pas";
    FILE_TOO_LARGE_MB_AT_MOST: "file too large ({detail} MB at most for an image)", "fichier trop volumineux (maximum {detail} Mo pour une image)";
    FOLDER_IS_NO_LONGER_ACCESSIBLE: "folder {path} is no longer accessible: {e}", "le dossier {path} n'est plus accessible : {e}";
    FOLDER_IS_NO_LONGER_ACCESSIBLE_2: "folder {path} is no longer accessible", "le dossier {path} n'est plus accessible";
    GIT_CANT_BE_FOUND_INSTALL_GIT: "Git can’t be found: install Git to use this view.", "Git est introuvable : installez Git pour utiliser cette vue.";
    GIT_DIDNT_ANSWER_IN_TIME: "Git didn’t answer in time.", "Git n'a pas répondu à temps.";
    GIT_REPOSITORY_NOT_ACCESSIBLE: "Git repository not accessible", "Dépôt Git inaccessible";
    GOES_THROUGH_A_BROKEN_SYMBOLIC_LINK: "`{path}` goes through a broken symbolic link: refused", "`{path}` passe par un lien symbolique cassé : refusé";
    IMAGE_TOO_LARGE_TO_SAVE_32: "image too large to save (32 MiB at most)", "image trop volumineuse pour être enregistrée (32 Mio au maximum)";
    INPUT_CONTEXT: "Input context", "Contexte d’entrée";
    INPUT_TOO_LONG_FOR_THE_TERMINAL: "Input too long for the terminal.", "Saisie trop longue pour le terminal.";
    INVALID_IDENTIFIER: "invalid {what} identifier", "identifiant de {what} invalide";
    INVALID_PATH_IT_MUST_BE_RELATIVE: "Invalid path: it must be relative to the project.", "Chemin invalide : il doit être relatif au projet.";
    INVALID_RANGE_THE_END_COMES_BEFORE: "invalid range: the end ({end}) comes before the start ({line})", "plage invalide : la fin ({end}) précède le début ({line})";
    INVALID_REVISION: "Invalid revision.", "Révision invalide.";
    ISNT_A_FILE: "`{shown}` isn’t a file", "`{shown}` n'est pas un fichier";
    ISNT_UTF_8_TEXT: "`{shown}` isn’t UTF-8 text", "`{shown}` n'est pas du texte UTF-8";
    IS_A_BINARY_FILE_NO_PREVIEW: "`{shown}` is a binary file: no preview available", "`{shown}` est un fichier binaire : aperçu indisponible";
    IS_NO_LONGER_ACCESSIBLE: "{what} is no longer accessible: {e}", "{what} n'est plus accessible : {e}";
    ITEM_NOT_FOUND: "item not found: {src}", "élément introuvable : {src}";
    KEEPING_THE_COMPUTER_AWAKE: "keeping the computer awake", "maintien de l’ordinateur actif";
    LEAVES_THE_AGENTS_WORKSPACE_REFUSED: "`{path}` leaves the agent’s workspace: refused", "`{path}` sort de l'espace de travail de l'agent : refusé";
    LEAVES_THE_PROJECT_THE_SYMBOLIC_LINK: "`{rel}` leaves the project: the symbolic link is refused", "`{rel}` sort du projet : le lien symbolique est refusé";
    LEAVES_THE_WORKSPACE_REFUSED: "`{path}` leaves the workspace: refused", "`{path}` sort de l'espace de travail : refusé";
    LINES_START_AT_1: "lines start at 1", "les lignes commencent à 1";
    LINE_IS_PAST_THE_END_OF: "line {line} is past the end of the file ({total} lines)", "ligne {line} au-delà de la fin du fichier ({total} lignes)";
    MAXIMUM_SUB_AGENT_DEPTH: "Maximum sub-agent depth", "Profondeur maximale des sous-agents";
    MESSAGE_ATTACHMENTS: "message attachments", "pièces jointes des messages";
    MINIMAL: "minimal", "minimale";
    MODEL_IDENTIFIER: "model identifier", "identifiant du modèle";
    MOVE_LOG: "move log", "journal du déplacement";
    NONE: "none", "aucune";
    NON_NORMALIZED_PATH_REFUSED: "non-normalized path refused", "chemin non normalisé refusé";
    NO_DETAILS: "no details", "aucun détail";
    NO_LONGER_EXISTS: "`{path}` no longer exists", "`{path}` n'existe plus";
    NO_LONGER_EXISTS_THE_FILE_WAS: "`{path}` no longer exists: the file was deleted or moved", "`{path}` n'existe plus : le fichier a été supprimé ou déplacé";
    NO_OPEN_PROJECT: "no open project", "aucun projet ouvert";
    OFF: "off", "désactivée";
    ONLY_REGULAR_FILES_ARE_COPIED: "only regular files are copied", "seuls les fichiers réguliers sont copiés";
    OPENING_THE_TERMINAL_WAS_INTERRUPTED: "Opening the terminal was interrupted ({e}).", "Ouverture du terminal interrompue ({e}).";
    OUTPUT_CONTEXT: "Output context", "Contexte de sortie";
    PERMISSIONS_ONLY_APPLY_TO_MACOS: "permissions only apply to macOS", "les autorisations ne concernent que macOS";
    POWERPOINT_PRESENTATION: "PowerPoint presentation", "présentation PowerPoint";
    PREFERENCES: "preferences ({file})", "préférences ({file})";
    PROMOTING_AN_ATTACHMENT: "promoting an attachment", "promotion d'une pièce jointe";
    PROVIDER_IDENTIFIER: "provider identifier", "identifiant du fournisseur";
    RAW_OUTPUTS: "Raw outputs", "Sorties brutes";
    READING: "reading `{path}`", "lecture de `{path}`";
    READING_2: "reading `{shown}`", "lecture de `{shown}`";
    REVISION_NOT_FOUND: "Revision not found: {rev}", "Révision introuvable : {rev}";
    RUN_RECORDS: "run records", "enregistrements des exécutions";
    SAFETY_MARGIN: "Safety margin", "Marge de sécurité";
    SKILL_LIST: "skill list: {e}", "liste des skills : {e}";
    SYSTEM_SETTINGS_COULD_NOT_OPEN: "System Settings could not open: {e}", "Réglages Système n’ont pas pu s’ouvrir : {e}";
    SYSTEM_SETTINGS_COULD_NOT_OPEN_2: "System Settings could not open", "Réglages Système n’ont pas pu s’ouvrir";
    TEXT: "{context}: {e}", "{context} : {e}";
    TEXT_FILE_TOO_LARGE_MB_AT: "text file too large ({detail} MB at most)", "fichier texte trop volumineux (maximum {detail} Mo)";
    THE_AGENTS_WORKSPACE_IS_NO_LONGER: "the agent’s workspace is no longer accessible: {e}", "l'espace de travail de l'agent n'est plus accessible : {e}";
    THE_AGENTS_WORKSPACE_IS_UNKNOWN: "the agent’s workspace is unknown", "l'espace de travail de l'agent est inconnu";
    THE_CONTENT_ISNT_AN_SVG_DOCUMENT: "the content isn’t an SVG document", "le contenu n'est pas un document SVG";
    THE_CONTENT_ISNT_A_PNG_IMAGE: "the content isn’t a PNG image", "le contenu n'est pas une image PNG";
    THE_MEMORY_IS_IN_USE_BY: "The memory is in use by a running run: wait for it to finish or stop it, then try again.", "La mémoire est utilisée par une exécution en cours : attendez sa fin ou arrêtez-la, puis recommencez.";
    THE_PROJECT_FOLDER: "the project folder", "le dossier du projet";
    THE_PROJECT_FOLDER_NO_LONGER_EXISTS: "The project folder no longer exists.", "Le dossier du projet n'existe plus.";
    THE_PROVIDERS_TOML_FILE_ISNT_UTF: "the providers.toml file isn’t UTF-8", "le fichier providers.toml n'est pas en UTF-8";
    THE_READING_FOLDER: "the reading folder", "le dossier de lecture";
    THE_SAVE_DIALOG_CLOSED_WITHOUT_AN: "the save dialog closed without an answer", "le dialogue d'enregistrement s'est fermé sans réponse";
    THE_TERMINAL_CANT_KEEP_UP_WITH: "The terminal can’t keep up with the input: try again.", "Le terminal ne suit pas la saisie : réessayez.";
    THE_TITLE_CANT_BE_EMPTY: "the title can’t be empty", "le titre ne peut pas être vide";
    THE_TITLE_EXCEEDS_CHARACTERS: "the title exceeds {TITLE_MAX_CHARS} characters", "le titre dépasse {TITLE_MAX_CHARS} caractères";
    THIS_AGENTS_ISOLATED_WORKSPACE_ISNT_RECORDED: "this agent’s isolated workspace isn’t recorded", "l'espace isolé de cet agent n'est pas enregistré";
    THIS_AGENTS_ISOLATED_WORKSPACE_WAS_DELETED: "this agent’s isolated workspace was deleted", "l'espace isolé de cet agent a été supprimé";
    THIS_ATTACHMENT_BELONGS_TO_A_SENT: "this attachment belongs to a sent message", "cette pièce jointe fait partie d'un message envoyé";
    THIS_CONVERSATION_HAS_NO_SUB_AGENTS: "this conversation has no sub-agents", "cette conversation n'a pas de sous-agents";
    THIS_CONVERSATION_ISNT_OPEN: "this conversation isn’t open", "cette conversation n'est pas ouverte";
    THIS_CONVERSATION_IS_OPEN_IN_ANOTHER: "this conversation is open in another project", "cette conversation est ouverte dans un autre projet";
    THIS_FOLDER_ISNT_A_GIT_REPOSITORY: "This folder isn’t a Git repository.", "Ce dossier n'est pas un dépôt Git.";
    THIS_ISNT_A_FOLDER: "this isn’t a folder", "ce n'est pas un dossier";
    THIS_PROJECT_IS_STILL_IN_THE: "this project is still in the list", "ce projet est encore dans la liste";
    TOO_MANY_CHANGES_TO_SHOW: "Too many changes to show.", "Trop de changements pour être affichés.";
    TO_SWITCH_CONVERSATIONS_OPEN_IT_A: "to switch conversations, open it: a session stays tied to its controller", "pour changer de conversation, ouvrez-la : une session reste liée à son contrôleur";
    UNKNOWN: "unknown", "inconnue";
    UNKNOWN_AGENT_IN_THIS_CONVERSATION: "unknown agent in this conversation", "agent inconnu dans cette conversation";
    UNKNOWN_ISOLATION_MODE: "unknown isolation mode: `{other}`", "mode d'isolation inconnu : `{other}`";
    UNKNOWN_PERMISSION_LEVEL: "unknown permission level: {level}", "niveau de permission inconnu : {level}";
    UNSUPPORTED_FORMAT_UTF_8_TEXT_PNG: "unsupported format: {what} (UTF-8 text, PNG, JPEG, GIF or WebP only)", "format non pris en charge : {what} (texte UTF-8, PNG, JPEG, GIF ou WebP seulement)";
    WORD_DOCUMENT: "Word document", "document Word";
    WRITING_THE_DIAGRAM: "writing the diagram", "écriture du diagramme";
    WRITING_THE_IMAGE: "writing the image", "écriture de l'image";
    ZIP_ARCHIVE: "ZIP archive", "archive ZIP";
    NO_IMAGE_INPUT: "This model doesn’t accept images: remove this attachment.", "Ce modèle n’accepte pas les images : retirez cette pièce jointe.";
    UNKNOWN_PROVIDER: "unknown provider in providers.toml", "fournisseur inconnu dans providers.toml";
    API_KEY_EMPTY: "the API key is empty", "la clé API est vide";
    KEYCHAIN_UNAVAILABLE: "the system's secure storage refused the key", "le stockage sécurisé du système a refusé la clé";
    UNKNOWN_MODEL: "unknown model in this provider", "modèle inconnu dans ce fournisseur";
    AN_EMPTY_POINTER_WOULD_REMOVE_THE: "an empty pointer would remove the whole request", "un pointeur vide supprimerait toute la requête";
    AN_EMPTY_VALUE_NULL_CANT_BE: "an empty value (null) can’t be written to providers.toml", "une valeur vide (null) ne peut pas être écrite dans providers.toml";
    A_POSITIVE_NUMBER_OR_ZERO_IS: "“{field}”: a positive number or zero is expected (for example 0.30)", "« {field} » : un nombre positif ou nul est attendu (par exemple 0.30)";
    A_REASONING_PROFILE_HAS_AN_EMPTY: "a reasoning profile has an empty identifier", "un profil de raisonnement a un identifiant vide";
    CACHE_COST: "cache cost", "coût du cache";
    CHANGE_LOG: "change log", "journal des modifications";
    CHANGE_LOG_PATCHES: "change log (patches)", "journal des modifications (correctifs)";
    CONVERSATION_FOLDER: "conversation folder", "dossier de la conversation";
    FIELD_CANT_BE_EMPTY: "field “{field}” can’t be empty", "le champ « {field} » ne peut pas être vide";
    INPUT_COST: "input cost", "coût d'entrée";
    ISNT_A_JSON_POINTER_IT_MUST: "“{pointer}” isn’t a JSON pointer (it must start with /)", "« {pointer} » n'est pas un pointeur JSON (il doit commencer par /)";
    IS_TOO_LARGE: "`{key}` is too large", "`{key}` est trop grand";
    NO_FOLDER_GIVEN: "no folder given", "aucun dossier indiqué";
    NO_FREE_FOLDER_FOR_THE_CONVERSATION: "no free folder for the conversation", "aucun dossier libre pour la conversation";
    NO_PATCH_SAVED_FOR_THIS_CHANGE: "no patch saved for this change", "aucun correctif enregistré pour cette modification";
    NUMBER_OUTSIDE_THE_RANGE_OF_TOML: "number outside the range of TOML integers", "nombre hors de l'intervalle des entiers TOML";
    OUTPUT_COST: "output cost", "coût de sortie";
    PROFILE_PARAMETER: "profile “{id}”, parameter “{key}”: {m}", "profil « {id} », paramètre « {key} » : {m}";
    PROFILE_PARAMETERS_MUST_BE_A_JSON: "profile “{id}”: parameters must be a JSON object", "profil « {id} » : les paramètres doivent être un objet JSON";
    PROFILE_PARAMETERS_THAT_CANT_BE_WRITTEN: "profile “{id}”: parameters that can’t be written: {m}", "profil « {id} » : paramètres non écrivables : {m}";
    PROJECTS_FILE_IS_UNREADABLE: "projects file {file} is unreadable: {e}", "fichier de projets {file} illisible : {e}";
    THE_CHARACTER_MUST_BE_WRITTEN_0: "“{pointer}”: the ~ character must be written ~0 or ~1", "« {pointer} » : le caractère ~ doit être écrit ~0 ou ~1";
    THE_NAME_CANT_BE_EMPTY: "the name can’t be empty", "le nom ne peut pas être vide";
    THE_NAME_EXCEEDS_CHARACTERS: "the name exceeds {MAX_NAME_CHARS} characters", "le nom dépasse {MAX_NAME_CHARS} caractères";
    THIS_FILE_HAS_NO_SAVED_PATCH: "this file has no saved patch", "ce fichier n'a pas de correctif enregistré";
    THIS_FOLDER_ALREADY_BELONGS_TO_ANOTHER: "this folder already belongs to another project", "ce dossier est déjà celui d'un autre projet";
    THIS_FOLDER_ALREADY_BELONGS_TO_PROJECT: "this folder already belongs to project “{name}”", "ce dossier est déjà celui du projet « {name} »";
    THIS_MODEL_HAS_NO_LIMITS_TABLE: "this model has no [limits] table", "ce modèle n'a pas de table [limits]";
    THIS_MODEL_PRICING_ISNT_A_TABLE: "this model: [pricing] isn’t a table", "ce modèle : [pricing] n'est pas une table";
    THIS_MODEL_REASONING_ISNT_A_TABLE: "this model: [reasoning] isn’t a table", "ce modèle : [reasoning] n'est pas une table";
    UNKNOWN_PROJECT: "unknown project", "projet inconnu";
    UNREADABLE_PROJECTS_FILE: "unreadable projects file ({e}): {detail}", "fichier de projets illisible ({e}) : {detail}";
    INVALID_CONVERSATION_IDENTIFIER: "invalid conversation identifier", "identifiant de conversation invalide";
    COULD_NOT_LAUNCH_GIT: "Could not launch Git ({e}).", "Impossible de lancer Git ({e}).";
    GIT_OUTPUT_UNAVAILABLE: "Git output unavailable.", "Sortie de Git indisponible.";
    COULD_NOT_LAUNCH_SHELL: "Could not launch {program}", "Impossible de lancer {program}";
    COVERAGE_NOTE: "Recorded: file changes applied by Bricks (write and edit tools, applied sub-agent changes). Not attributed: the effects of shell commands, MCP tools and changes made outside Bricks.", "Enregistrées : les modifications de fichiers appliquées par Bricks (outils d'écriture et d'édition, changements de sous-agents appliqués). Non attribuées : les effets des commandes shell, des outils MCP et des modifications faites hors de Bricks.";
    UNKNOWN_PROJECTS_FILE_VERSION: "{file}: projects file version {schema} not recognised; the projects aren’t changed", "{file} : version {schema} du fichier de projets non reconnue ; les projets ne sont pas modifiés";
    KEPT_AS: "kept as {aside}", "conservé sous {aside}";
    COULD_NOT_BE_SET_ASIDE: "{file} could not be set aside", "{file} n'a pas pu être mis de côté";
    PROVIDERS_FILE_CHANGED: "the providers.toml file changed since it was loaded: reload the list before editing", "le fichier providers.toml a changé depuis son chargement : rechargez la liste avant de modifier";
    INVALID_ID_CHARACTERS: "“{field}”: letters, digits, “.”, “_” and “-” only, starting with a letter or digit", "« {field} » : lettres, chiffres, « . », « _ » et « - » seulement, en commençant par une lettre ou un chiffre";
    INLINE_PROVIDERS_UNSUPPORTED: "providers.toml: providers and models must be written as [[providers]] and [[providers.models]] tables to be edited from the app", "providers.toml : les fournisseurs et les modèles doivent être écrits en tables [[providers]] et [[providers.models]] pour être modifiés depuis l'application";
    DUPLICATE_PROFILE_ID: "duplicate profile identifier: “{id}”", "identifiant de profil en double : « {id} »";
    UNKNOWN_LIMIT: "unknown", "inconnu";
    UNKNOWN_CONVERSATION: "unknown conversation", "conversation inconnue";
    INVALID_PATH: "invalid path", "chemin invalide";
    INVALID_TOML: "{origin}: invalid TOML ({position})", "{origin} : TOML invalide ({position})";
    UNKNOWN_CHANGE: "unknown change", "modification inconnue";
    INVALID_ID: "invalid identifier", "identifiant invalide";
    SAVE_PATH: "save path", "chemin d'enregistrement";
    ABSOLUTE_PATH_REQUIRED: "absolute path required", "chemin absolu requis";
    INVALID_COMMAND: "invalid command: {e}", "commande invalide : {e}";
    OLD_OFFICE_DOCUMENT: "old Office document (Word, Excel, PowerPoint)", "document Office ancien (Word, Excel, PowerPoint)";
    NON_UTF8_TEXT: "non-UTF-8 text", "texte non UTF-8";
    DRAFT: "draft", "brouillon";
    PROMOTION: "promotion", "promotion";
    TERMINAL_GONE: "This terminal no longer exists.", "Ce terminal n'existe plus.";
    PATCH: "patch", "correctif";
    UNREADABLE_PATCH: "unreadable patch", "correctif illisible";
    RENAMING: "renaming", "renommage";
    CONVERSATIONS: "conversations", "conversations";
    CONVERSATION: "conversation", "conversation";
    NO_ANSWER: "no answer", "pas de réponse";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue, read from this file: one `NAME: "english", "french";` per message. The
    /// catalogue has no escaped quote, so the line can be split at the quote marks.
    fn catalogue() -> Vec<(String, String, String)> {
        let mut found = Vec::new();
        for line in include_str!("i18n.rs").lines() {
            let line = line.trim();
            let Some((name, rest)) = line.split_once(": \"") else {
                continue;
            };
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                continue;
            }
            let Some(rest) = rest.strip_suffix("\";") else {
                continue;
            };
            let Some((en, fr)) = rest.split_once("\", \"") else {
                continue;
            };
            found.push((name.to_string(), en.to_string(), fr.to_string()));
        }
        found
    }

    /// The `{names}` of a text, sorted and without repeats.
    fn placeholders(text: &str) -> Vec<String> {
        let mut names: Vec<String> = text
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_string()))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    #[test]
    fn the_catalogue_is_read_in_full() {
        // Guards the parser of the test below: a silent miss would make it check nothing.
        assert!(catalogue().len() >= 150, "{} messages", catalogue().len());
    }

    #[test]
    fn every_message_has_a_text_in_both_languages() {
        for (name, en, fr) in catalogue() {
            assert!(!en.trim().is_empty(), "{name}: empty English text");
            assert!(!fr.trim().is_empty(), "{name}: empty French text");
        }
    }

    #[test]
    fn both_languages_hold_the_same_placeholders() {
        for (name, en, fr) in catalogue() {
            assert_eq!(placeholders(&fr), placeholders(&en), "{name}");
        }
    }

    #[test]
    fn render_fills_the_parameters_in_the_current_language() {
        let message = Message {
            en: "{count} files in {path}",
            fr: "{count} fichiers dans {path}",
        };
        // `set_language` is global and the tests run in parallel: either language may be current.
        let shown = tr!(message, count = 3, path = "src");
        assert!(
            shown == "3 files in src" || shown == "3 fichiers dans src",
            "{shown}"
        );
        let plain = Message {
            en: "no parameter",
            fr: "sans paramètre",
        };
        let shown = render(plain, &[]);
        assert!(
            shown == "no parameter" || shown == "sans paramètre",
            "{shown}"
        );
    }

    #[test]
    fn a_catalogue_message_is_the_one_of_its_name() {
        let shown = render(msg::NO_DETAILS, &[]);
        assert!(
            shown == msg::NO_DETAILS.en || shown == msg::NO_DETAILS.fr,
            "{shown}"
        );
    }
}
