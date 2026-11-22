//! TVDB names languages by ISO 639-2/T (`fra`), everything else here by ISO 639-1 (`fr`).

/// The ISO 639-1 languages by their 639-2/T code.
const ISO_639_2T: &[(&str, &str)] = &[
    ("aar", "aa"),
    ("abk", "ab"),
    ("afr", "af"),
    ("aka", "ak"),
    ("amh", "am"),
    ("ara", "ar"),
    ("arg", "an"),
    ("asm", "as"),
    ("ava", "av"),
    ("ave", "ae"),
    ("aym", "ay"),
    ("aze", "az"),
    ("bak", "ba"),
    ("bam", "bm"),
    ("bel", "be"),
    ("ben", "bn"),
    ("bis", "bi"),
    ("bod", "bo"),
    ("bos", "bs"),
    ("bre", "br"),
    ("bul", "bg"),
    ("cat", "ca"),
    ("ces", "cs"),
    ("cha", "ch"),
    ("che", "ce"),
    ("chu", "cu"),
    ("chv", "cv"),
    ("cor", "kw"),
    ("cos", "co"),
    ("cre", "cr"),
    ("cym", "cy"),
    ("dan", "da"),
    ("deu", "de"),
    ("div", "dv"),
    ("dzo", "dz"),
    ("ell", "el"),
    ("eng", "en"),
    ("epo", "eo"),
    ("est", "et"),
    ("eus", "eu"),
    ("ewe", "ee"),
    ("fao", "fo"),
    ("fas", "fa"),
    ("fij", "fj"),
    ("fin", "fi"),
    ("fra", "fr"),
    ("fry", "fy"),
    ("ful", "ff"),
    ("gla", "gd"),
    ("gle", "ga"),
    ("glg", "gl"),
    ("glv", "gv"),
    ("grn", "gn"),
    ("guj", "gu"),
    ("hat", "ht"),
    ("hau", "ha"),
    ("heb", "he"),
    ("her", "hz"),
    ("hin", "hi"),
    ("hmo", "ho"),
    ("hrv", "hr"),
    ("hun", "hu"),
    ("hye", "hy"),
    ("ibo", "ig"),
    ("ido", "io"),
    ("iii", "ii"),
    ("iku", "iu"),
    ("ile", "ie"),
    ("ina", "ia"),
    ("ind", "id"),
    ("ipk", "ik"),
    ("isl", "is"),
    ("ita", "it"),
    ("jav", "jv"),
    ("jpn", "ja"),
    ("kal", "kl"),
    ("kan", "kn"),
    ("kas", "ks"),
    ("kat", "ka"),
    ("kau", "kr"),
    ("kaz", "kk"),
    ("khm", "km"),
    ("kik", "ki"),
    ("kin", "rw"),
    ("kir", "ky"),
    ("kom", "kv"),
    ("kon", "kg"),
    ("kor", "ko"),
    ("kua", "kj"),
    ("kur", "ku"),
    ("lao", "lo"),
    ("lat", "la"),
    ("lav", "lv"),
    ("lim", "li"),
    ("lin", "ln"),
    ("lit", "lt"),
    ("ltz", "lb"),
    ("lub", "lu"),
    ("lug", "lg"),
    ("mah", "mh"),
    ("mal", "ml"),
    ("mar", "mr"),
    ("mkd", "mk"),
    ("mlg", "mg"),
    ("mlt", "mt"),
    ("mon", "mn"),
    ("mri", "mi"),
    ("msa", "ms"),
    ("mya", "my"),
    ("nau", "na"),
    ("nav", "nv"),
    ("nbl", "nr"),
    ("nde", "nd"),
    ("ndo", "ng"),
    ("nep", "ne"),
    ("nld", "nl"),
    ("nno", "nn"),
    ("nob", "nb"),
    ("nor", "no"),
    ("nya", "ny"),
    ("oci", "oc"),
    ("oji", "oj"),
    ("ori", "or"),
    ("orm", "om"),
    ("oss", "os"),
    ("pan", "pa"),
    ("pli", "pi"),
    ("pol", "pl"),
    ("por", "pt"),
    ("pus", "ps"),
    ("que", "qu"),
    ("roh", "rm"),
    ("ron", "ro"),
    ("run", "rn"),
    ("rus", "ru"),
    ("sag", "sg"),
    ("san", "sa"),
    ("sin", "si"),
    ("slk", "sk"),
    ("slv", "sl"),
    ("sme", "se"),
    ("smo", "sm"),
    ("sna", "sn"),
    ("snd", "sd"),
    ("som", "so"),
    ("sot", "st"),
    ("spa", "es"),
    ("sqi", "sq"),
    ("srd", "sc"),
    ("srp", "sr"),
    ("ssw", "ss"),
    ("sun", "su"),
    ("swa", "sw"),
    ("swe", "sv"),
    ("tah", "ty"),
    ("tam", "ta"),
    ("tat", "tt"),
    ("tel", "te"),
    ("tgk", "tg"),
    ("tgl", "tl"),
    ("tha", "th"),
    ("tir", "ti"),
    ("ton", "to"),
    ("tsn", "tn"),
    ("tso", "ts"),
    ("tuk", "tk"),
    ("tur", "tr"),
    ("twi", "tw"),
    ("uig", "ug"),
    ("ukr", "uk"),
    ("urd", "ur"),
    ("uzb", "uz"),
    ("ven", "ve"),
    ("vie", "vi"),
    ("vol", "vo"),
    ("wln", "wa"),
    ("wol", "wo"),
    ("xho", "xh"),
    ("yid", "yi"),
    ("yor", "yo"),
    ("zha", "za"),
    ("zho", "zh"),
    ("zul", "zu"),
];

/// `fra` → `fr`. A code already of two letters passes as it is; one with no 639-1 equivalent
/// (`yue`, Cantonese) is kept whole rather than lost, as a language tag may be.
pub(crate) fn from_tvdb(code: &str) -> String {
    let code = code.to_ascii_lowercase();
    match code.as_str() {
        // TVDB's own codes for two written forms ISO does not separate.
        "zhtw" => "zh-TW".to_owned(),
        "pt" => "pt-BR".to_owned(),
        _ => ISO_639_2T
            .binary_search_by(|(three, _)| three.cmp(&code.as_str()))
            .map_or(code.clone(), |at| ISO_639_2T[at].1.to_owned()),
    }
}

/// `fr-FR` → `fra`: how TVDB names the language of a tag. `None` for a language TVDB cannot
/// be asked in by this table.
pub(crate) fn to_tvdb(tag: &str) -> Option<String> {
    let primary = tag.split('-').next().unwrap_or(tag).to_ascii_lowercase();
    ISO_639_2T
        .iter()
        .find(|(_, two)| *two == primary)
        .map(|(three, _)| (*three).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tvdb_codes() {
        assert_eq!(from_tvdb("fra"), "fr");
        assert_eq!(from_tvdb("eng"), "en");
        assert_eq!(from_tvdb("jpn"), "ja");
        assert_eq!(from_tvdb("zhtw"), "zh-TW");
        assert_eq!(from_tvdb("yue"), "yue");
        assert_eq!(to_tvdb("fr-FR").as_deref(), Some("fra"));
    }

    #[test]
    fn the_table_is_sorted_for_binary_search() {
        assert!(ISO_639_2T.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }
}
