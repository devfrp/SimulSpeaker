//! Langue de l'interface : français (texte source) ou anglais.
//!
//! Le code de l'interface est écrit en français ; [`tr`] renvoie la traduction
//! anglaise quand la langue active est `en`. Les recherches ignorent la casse et
//! conservent les majuscules (`tr("PRÉRÉGLAGES")` → `"PRESETS"`). Les messages
//! venant du moteur (progression, erreurs) passent par [`tr_message`], qui sait
//! traduire un préfixe (« Mesure CEA-2034 : KEF R3 ») ou un motif.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::Regex;

pub const LANGUAGES: &[(&str, &str)] = &[("fr", "Français"), ("en", "English")];

static ENGLISH: AtomicBool = AtomicBool::new(false);

const EN: &[(&str, &str)] = &[
    // --- barre supérieure, état
    ("Auto", "Auto"),
    ("Appliquer automatiquement chaque changement pendant la simulation\n(bascule sans coupure)",
     "Apply every change automatically while the simulation runs\n(seamless switch)"),
    ("Réglages", "Settings"),
    ("Déplier / replier les réglages (les onglets restent visibles)",
     "Expand / collapse the settings (tabs stay visible)"),
    ("Exporter", "Export"),
    ("Enregistrer les filtres (WAV, true stereo, Equalizer APO)",
     "Save the filters (WAV, true stereo, Equalizer APO)"),
    ("Appliquer", "Apply"),
    ("Appliquer •", "Apply •"),
    ("Arrêter la simulation", "Stop the simulation"),
    ("Démarrer la simulation", "Start the simulation"),
    ("Langue de l'interface", "Interface language"),
    ("Apparence : comme le système, claire ou sombre", "Appearance: like the system, light or dark"),
    ("Clair", "Light"),
    ("Sombre", "Dark"),
    ("○ VEILLE", "○ STANDBY"),
    ("PRÊT", "READY"),
    ("ERREUR", "ERROR"),
    ("SIMULATION ARRÊTÉE", "SIMULATION STOPPED"),
    ("FILTRES APPLIQUÉS", "FILTERS APPLIED"),
    ("FILTRES GÉNÉRÉS", "FILTERS BUILT"),
    ("AUTO-DÉMARRAGE ACTIVÉ", "AUTOSTART ENABLED"),
    ("AUTO-DÉMARRAGE DÉSACTIVÉ", "AUTOSTART DISABLED"),
    ("Arrêt…", "Stopping…"),
    ("Démarrage…", "Starting…"),
    ("Application automatique…", "Applying automatically…"),
    ("Génération des filtres…", "Building filters…"),
    ("Export des filtres…", "Exporting filters…"),
    ("Configuration de l'auto-démarrage…", "Configuring autostart…"),
    ("Index AutoEq…", "AutoEq index…"),
    // --- sources
    ("01 Casque", "01 Headphones"),
    ("02 Enceintes", "02 Speakers"),
    ("Casque", "Headphones"),
    ("Enceintes", "Speakers"),
    ("Aucun casque", "No headphones"),
    ("sans casque", "no headphones"),
    ("SANS CORRECTION", "NO CORRECTION"),
    ("Rechercher parmi 8 850 mesures…", "Search 8,850 measurements…"),
    ("chargement de l'index autoeq…", "loading the autoeq index…"),
    ("Correction", "Correction"),
    ("Appliquer la correction du casque (AutoEq ou import)",
     "Apply the headphone correction (AutoEq or import)"),
    ("Importer…", "Import…"),
    ("Votre mesure (fréquence, dB : REW, Squiglink, CSV AutoEq)\nou un EQ AutoEq / Equalizer APO (ParametricEQ, GraphicEQ)",
     "Your measurement (frequency, dB: REW, Squiglink, AutoEq CSV)\nor an AutoEq / Equalizer APO EQ (ParametricEQ, GraphicEQ)"),
    ("Supprimer cet import", "Remove this import"),
    ("Toutes", "All"),
    ("moniteur de studio", "studio monitor"),
    ("hi-fi", "hi-fi"),
    ("bibliothèque", "bookshelf"),
    ("colonne", "floorstander"),
    ("centrale", "centre"),
    (" (CEA-2034 calculé depuis les mesures polaires)", " (CEA-2034 computed from polar data)"),
    // --- plan et analyse
    ("PLAN", "PLAN"),
    ("Pièce personnalisée", "Custom room"),
    ("nuage", "cloud"),
    ("Placement impossible dans cette pièce", "This placement does not fit in the room"),
    ("pièce désactivée", "room disabled"),
    ("réflexions désactivées (ordre : aucune)", "reflections disabled (order: none)"),
    ("glissez l'auditeur ou les enceintes  ·  — direct  ┄ réflexions",
     "drag the listener or the speakers  ·  — direct  ┄ reflections"),
    ("Réponse simulée", "Simulated response"),
    ("Réverbération", "Reverberation"),
    ("« Appliquer » pour calculer la réponse simulée", "“Apply” to compute the simulated response"),
    ("bande claire : 0,2–0,4 s, écoute critique", "light band: 0.2–0.4 s, critical listening"),
    ("enceinte", "speaker"),
    ("casque", "headphones"),
    ("oreille", "ear"),
    ("traitement par paroi", "per-wall treatment"),
    // --- réglages
    ("Salle", "Room"),
    ("Placement et réflexions", "Placement and reflections"),
    ("Écoute et avancé", "Listening and advanced"),
    ("Activer / désactiver — affiche ou retire les réglages",
     "Enable / disable — shows or hides the settings"),
    ("Pièce désactivée : écoute en champ libre, sans réflexions ni réverbération.",
     "Room disabled: free-field listening, no reflections or reverberation."),
    ("préréglages", "presets"),
    ("RÉGLAGES PERSONNALISÉS", "CUSTOM SETTINGS"),
    ("Salon", "Living room"),
    ("Studio traité", "Treated studio"),
    ("Régie mate", "Dead control room"),
    ("Home-cinéma", "Home cinema"),
    ("Chambre", "Bedroom"),
    ("Pièce nue", "Bare room"),
    ("Traitement", "Treatment"),
    ("Absorbeurs", "Absorbers"),
    ("Mousse", "Foam"),
    ("Laine 10", "Wool 10"),
    ("Laine 20", "Wool 20"),
    ("Mousse 5 cm · laine minérale 10 cm · laine 20 cm (large bande, absorbe aussi le grave)",
     "5 cm foam · 10 cm mineral wool · 20 cm wool (broadband, also absorbs bass)"),
    ("Paroi par paroi", "Per wall"),
    ("Pièges à basses (angles)", "Bass traps (corners)"),
    ("Aucun", "None"),
    ("Quelques", "A few"),
    ("Complet", "Full"),
    ("Murs traités", "Treated walls"),
    ("Plafond", "Ceiling"),
    ("Plâtre", "Plaster"),
    ("Nuage", "Cloud"),
    ("Mur avant", "Front wall"),
    ("Murs latéraux", "Side walls"),
    ("Mur arrière", "Rear wall"),
    ("Matériaux", "Materials"),
    ("Murs", "Walls"),
    ("Maçonnerie", "Masonry"),
    ("Placo", "Drywall"),
    ("Sol", "Floor"),
    ("Carrelage", "Tile"),
    ("Parquet", "Wood"),
    ("Tapis", "Rug"),
    ("Moquette", "Carpet"),
    ("Mobilier", "Furniture"),
    ("Vide", "Empty"),
    ("Meublé", "Furnished"),
    ("Très", "Full"),
    ("Vide · meublé · très meublé", "Empty · furnished · fully furnished"),
    ("Dimensions", "Dimensions"),
    ("Longueur", "Length"),
    ("Largeur", "Width"),
    ("Hauteur sous plafond", "Ceiling height"),
    ("Réflexions", "Reflections"),
    ("Désactivées : seule la réverbération tardive reste.", "Disabled: only the late reverberation remains."),
    ("Ordre (rebonds simulés)", "Order (simulated bounces)"),
    ("Diffusion du mur arrière", "Rear-wall diffusion"),
    ("Placement", "Placement"),
    ("Angle des enceintes", "Speaker angle"),
    ("Distance d'écoute", "Listening distance"),
    ("Enceintes – mur avant", "Speakers – front wall"),
    ("Décalage latéral", "Lateral offset"),
    ("centré", "centred"),
    ("Hauteur des oreilles", "Ear height"),
    ("Hauteur des tweeters", "Tweeter height"),
    ("aux oreilles", "at ear level"),
    ("gauche", "left"),
    ("droite", "right"),
    ("centre", "centre"),
    ("Écoute", "Listening"),
    ("Tonalité et balance désactivées (réponse neutre).", "Tone and balance disabled (neutral response)."),
    ("Basses (105 Hz)", "Bass (105 Hz)"),
    ("Aigus (10 kHz)", "Treble (10 kHz)"),
    ("Balance", "Balance"),
    ("Préampli", "Preamp"),
    ("Avancé", "Advanced"),
    ("Réglages avancés masqués.", "Advanced settings hidden."),
    ("Quantité de pièce", "Room amount"),
    ("RT60 imposé (500 Hz–1 kHz)", "Manual RT60 (500 Hz–1 kHz)"),
    ("RT60 calculé (matériaux)", "RT60 from materials"),
    ("Égalisation champ diffus", "Diffuse-field EQ"),
    ("Fréquence d'échantillonnage", "Sample rate"),
    ("Auto (PipeWire / 48 kHz)", "Auto (PipeWire / 48 kHz)"),
    ("44,1 kHz", "44.1 kHz"),
    ("Sortie", "Output"),
    ("Casque (sortie physique)", "Headphones (physical output)"),
    ("Actualiser la liste des sorties", "Refresh the output list"),
    ("Démarrer à la connexion", "Start at login"),
    ("Auto (sortie par défaut)", "Auto (default output)"),
    ("Traitement désactivé : parois nues, sans panneaux, pièges ni nuage.",
     "Treatment disabled: bare surfaces, no panels, traps or cloud."),
    ("Réflexions précoces désactivées : seule la réverbération tardive reste.",
     "Early reflections disabled: only the late reverberation remains."),
    ("Réverbération tardive désactivée : seules les réflexions précoces restent.",
     "Late reverberation disabled: only the early reflections remain."),
    ("Placement standard : triangle stéréo à 30°, 2 m, auditeur centré.",
     "Standard placement: stereo triangle at 30°, 2 m, centred listener."),
    ("Correction du casque désactivée : réponse brute du casque.",
     "Headphone correction disabled: raw headphone response."),
    ("Valeurs par défaut : égalisation champ diffus, fréquence automatique.",
     "Defaults: diffuse-field EQ, automatic sample rate."),
    ("Simulation arrêtée : le son va directement au casque.",
     "Simulation stopped: sound goes straight to the headphones."),
    ("Sans effet : la pièce est désactivée (onglet Salle).", "No effect: the room is disabled (Room tab)."),
    ("dimensions", "dimensions"),
    ("matériaux", "materials"),
    ("intensité", "amount"),
    ("absorbeurs", "absorbers"),
    ("couverture", "coverage"),
    ("grave", "bass"),
    ("ordre", "order"),
    ("diffusion", "diffusion"),
    ("temps de réverbération", "reverberation time"),
    ("correction", "correction"),
    ("hrtf", "hrtf"),
    ("fréquence d'échantillonnage", "sample rate"),
    ("Importer une mesure…", "Import a measurement…"),
    ("La réverbération tardive est un champ diffus dont la durée suit le RT60 de chaque octave (vu-mètres « Réverbération »).",
     "The late reverberation is a diffuse field whose decay follows the RT60 of each octave (“Reverberation” meters)."),
    ("Correction AutoEq de votre modèle (ou de votre propre mesure importée), appliquée avant la simulation.",
     "AutoEq correction for your model (or your own imported measurement), applied before the simulation."),
    // --- boîtes de dialogue
    ("Importer une mesure ou un EQ de casque", "Import a headphone measurement or EQ"),
    ("Mesures et EQ", "Measurements and EQs"),
    ("Tous les fichiers", "All files"),
    ("Nom du casque", "Headphone name"),
    ("Nom affiché :", "Display name:"),
    ("Courbe cible", "Target curve"),
    ("Mesure brute détectée : vers quelle cible corriger ?\n(Harman over-ear = cible des corrections AutoEq de casques circum-auriculaires)",
     "Raw measurement detected: which target should it be corrected to?\n(Harman over-ear = target of AutoEq corrections for over-ear headphones)"),
    ("Dossier d'export des filtres", "Folder for the exported filters"),
    ("Annuler", "Cancel"),
    ("Importer", "Import"),
    ("Fermer", "Close"),
    // --- gabarits (format)
    ("{shown} / {total} mesures", "{shown} / {total} measurements"),
    ("INDEX INDISPONIBLE : {msg}", "INDEX UNAVAILABLE: {msg}"),
    ("Filtrer {n} modèles…", "Filter {n} models…"),
    ("{cat} · {kind} · {type}\nscore Olive {score} ({wsub} avec caisson) · LFX {lfx} Hz\nmesure : {origin}, Klippel NFS{computed}",
     "{cat} · {kind} · {type}\nOlive score {score} ({wsub} with subwoofer) · LFX {lfx} Hz\nmeasurement: {origin}, Klippel NFS{computed}"),
    ("mur avant · {w} m", "front wall · {w} m"),
    ("{l} m × {w} m × h {h} m  ·  {v} m³  ·  oreilles {e} m", "{l} m × {w} m × h {h} m  ·  {v} m³  ·  ears {e} m"),
    (" · tweeters {t} m", " · tweeters {t} m"),
    ("traitement {p} %", "treatment {p} %"),
    ("PIPEWIRE INDISPONIBLE : {e}", "PIPEWIRE UNAVAILABLE: {e}"),
    ("À L'ARRÊT : RETOUR SUR {sink}", "ON STOP: BACK TO {sink}"),
    ("INDEX AUTOEQ CHARGÉ · {n} MESURES", "AUTOEQ INDEX LOADED · {n} MEASUREMENTS"),
    ("CASQUE IMPORTÉ : {x}", "HEADPHONES IMPORTED: {x}"),
    ("{n} FICHIERS EXPORTÉS VERS {folder}", "{n} FILES EXPORTED TO {folder}"),
    ("CONFIGURATION {slot}", "CONFIGURATION {slot}"),
    ("Lecture impossible : {e}", "Cannot read the file: {e}"),
    ("Initialisation impossible : {e}", "Cannot start the application: {e}"),
    ("{d} m {side}", "{d} m {side}"),
    ("{v} dB {side}", "{v} dB {side}"),
    // --- messages du moteur et de l'application
    ("Mesure CEA-2034", "CEA-2034 measurement"),
    ("Correction du casque", "Headphone correction"),
    ("HRTF KEMAR", "KEMAR HRTF"),
    ("Filtres du haut-parleur", "Speaker filters"),
    ("Champ diffus du HRTF", "HRTF diffuse field"),
    ("Normalisation", "Normalisation"),
    ("Terminé", "Done"),
    ("Écriture des filtres", "Writing filters"),
    ("Filtres prêts", "Filters ready"),
    ("Bascule sans coupure", "Seamless switch"),
    ("Démarrage du service", "Starting the service"),
    ("Aucun casque sélectionné : pas de correction AutoEq", "No headphones selected: no AutoEq correction"),
    ("Attention : crête d'IR > 0 dBFS", "Warning: IR peak above 0 dBFS"),
    ("Démarrage impossible", "Cannot start"),
    ("Arrêt impossible", "Cannot stop"),
    ("Auto-démarrage", "Autostart"),
    ("Outils manquants", "Missing tools"),
    ("Haut-parleur inconnu", "Unknown speaker"),
    ("Cible inconnue", "Unknown target"),
    ("Fichier importé introuvable", "Imported file not found"),
    ("Lecture impossible", "Cannot read the file"),
    ("Le service s'est arrêté aussitôt.", "The service stopped right away."),
    ("Auto-démarrage disponible uniquement sous Linux", "Autostart is only available on Linux"),
    ("La simulation en direct nécessite Linux + PipeWire. Utilisez « Exporter » pour un convolveur (Equalizer APO…).",
     "Live simulation needs Linux + PipeWire. Use “Export” for a convolver (Equalizer APO…)."),
    ("Aucun filtre « Filter N: ON … » reconnu dans ce fichier", "No “Filter N: ON …” line found in this file"),
    ("Mesure illisible : il faut au moins 10 lignes « fréquence, dB »",
     "Unreadable measurement: at least 10 “frequency, dB” lines are needed"),
    ("Mesure incomplète : elle doit couvrir au moins 200 Hz – 5 kHz",
     "Incomplete measurement: it must cover at least 200 Hz – 5 kHz"),
    // --- onglet Casque, comparaison A/B, interrupteurs
    ("mesure", "measurement"),
    ("Autres mesures de ce modèle", "Other measurements of this model"),
    ("Même casque mesuré par une autre source ou sur un autre banc",
     "Same headphones measured by another source or on another rig"),
    ("Cible de la correction", "Correction target"),
    ("Courbe vers laquelle votre mesure brute est corrigée", "Curve your raw measurement is corrected to"),
    ("Une seule mesure disponible pour ce modèle.", "Only one measurement is available for this model."),
    ("préampli {v} dB", "preamp {v} dB"),
    ("EQ graphique · {n} points", "graphic EQ · {n} points"),
    ("{n} filtres", "{n} filters"),
    ("Comparaison A/B : deux configurations complètes (enceintes, pièce,\nplacement, écoute). Basculer applique l'autre sans coupure.",
     "A/B comparison: two complete setups (speakers, room,\nplacement, listening). Switching applies the other one without dropouts."),
    ("Activer / désactiver « {tab} ».", "Turn “{tab}” on / off."),
    ("Coupé : {what}", "Off: {what}"),
];

static TABLE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| EN.iter().copied().collect());
static CASEFOLD: LazyLock<HashMap<String, &'static str>> =
    LazyLock::new(|| EN.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect());

// Motifs (messages contenant des valeurs) : expression française → gabarit anglais.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"Haut-parleur (\w) : sons directs et réflexions", "Speaker {0}: direct sound and reflections"),
        (r"Haut-parleur (\w) : réverbération", "Speaker {0}: reverberation"),
        (r"RT60 ([\d.]+) s \(500 Hz–1 kHz\), distance critique ([\d.]+) m, (\d+) sources-images/enceinte",
         "RT60 {0} s (500 Hz–1 kHz), critical distance {1} m, {2} image sources per speaker"),
    ]
    .into_iter()
    .map(|(re, t)| (Regex::new(re).expect("motif valide"), t))
    .collect()
});

/// Langue du système : variables `LC_ALL`, `LC_MESSAGES`, `LANG`, sinon réglage de l'OS.
pub fn system_language() -> &'static str {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return if value.to_lowercase().starts_with("fr") { "fr" } else { "en" };
            }
        }
    }
    match sys_locale::get_locale() {
        Some(code) if code.to_lowercase().starts_with("fr") => "fr",
        _ => "en",
    }
}

/// Active `fr` ou `en` ; une valeur vide ou inconnue suit le système.
pub fn set_language(code: &str) -> &'static str {
    let lang = match code {
        "fr" => "fr",
        "en" => "en",
        _ => system_language(),
    };
    ENGLISH.store(lang == "en", Ordering::Relaxed);
    lang
}

pub fn language() -> &'static str {
    if ENGLISH.load(Ordering::Relaxed) { "en" } else { "fr" }
}

/// Comme `str.isupper()` de Python : au moins une lettre à casse, toutes majuscules.
fn is_upper(text: &str) -> bool {
    let mut cased = false;
    for c in text.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            cased = true;
        }
    }
    cased
}

pub fn tr(text: &str) -> String {
    if !ENGLISH.load(Ordering::Relaxed) || text.is_empty() {
        return text.to_string();
    }
    if let Some(v) = TABLE.get(text) {
        return (*v).to_string();
    }
    match CASEFOLD.get(&text.to_lowercase()) {
        Some(found) if is_upper(text) => found.to_uppercase(),
        Some(found) => (*found).to_string(),
        None => text.to_string(),
    }
}

/// Remplace `{clé}` par les valeurs données.
pub fn format_template(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in values {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Gabarit traduit puis complété : `trf("{n} FICHIERS…", &[("n", "3")])`.
pub fn trf(template: &str, values: &[(&str, &str)]) -> String {
    format_template(&tr(template), values)
}

/// Message du moteur : texte exact, motif connu, ou préfixe « Préfixe : reste ».
pub fn tr_message(message: &str) -> String {
    if !ENGLISH.load(Ordering::Relaxed) || message.is_empty() {
        return message.to_string();
    }
    let direct = tr(message);
    if direct != message {
        return direct;
    }
    for (pattern, template) in PATTERNS.iter() {
        if let Some(caps) = pattern.captures(message) {
            let whole = caps.get(0).expect("groupe 0");
            let mut t = template.to_string();
            for i in 1..caps.len() {
                t = t.replace(&format!("{{{}}}", i - 1), caps.get(i).map_or("", |m| m.as_str()));
            }
            return format!("{}{}{}", &message[..whole.start()], t, &message[whole.end()..]);
        }
    }
    if let Some((head, rest)) = message.split_once(" : ") {
        let head_tr = tr(head);
        if head_tr != head {
            return format!("{head_tr}: {}", tr_message(rest));
        }
    }
    if message.contains('\n') {
        return message.split('\n').map(tr_message).collect::<Vec<_>>().join("\n");
    }
    message.to_string()
}

/// Nombre avec la virgule (fr) ou le point (en) décimal.
pub fn num(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if ENGLISH.load(Ordering::Relaxed) { text } else { text.replace('.', ",") }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Un seul test : la langue est un état global partagé.
    #[test]
    fn translations() {
        set_language("en");
        assert_eq!(tr("PRÉRÉGLAGES"), "PRESETS");
        assert_eq!(tr("Salon"), "Living room");
        assert_eq!(tr("inconnu"), "inconnu");
        assert_eq!(tr_message("Mesure CEA-2034 : KEF R3"), "CEA-2034 measurement: KEF R3");
        assert_eq!(tr_message("Haut-parleur L : réverbération"), "Speaker L: reverberation");
        assert_eq!(
            tr_message("RT60 0.35 s (500 Hz–1 kHz), distance critique 1.20 m, 25 sources-images/enceinte"),
            "RT60 0.35 s (500 Hz–1 kHz), critical distance 1.20 m, 25 image sources per speaker"
        );
        assert_eq!(trf("{n} filtres", &[("n", "3")]), "3 filters");
        assert_eq!(num(1.26, 1), "1.3");
        set_language("fr");
        assert_eq!(tr("Salon"), "Salon");
        assert_eq!(num(2.5, 2), "2,50");
        assert_eq!(language(), "fr");
    }
}
