//! The `Language` enum of the official API, the domain's own value for value.
//!
//! **Generated** by `python tools/gen_languages.py` from CLDR 39. Do not edit; regenerate.

use async_graphql::Enum;

/// A language, by its shortest ISO 639 code: ISO 639-1 where it has one (`FR`), else
/// ISO 639-2 or 639-3 (`FIL`, `YUE`). `UND`, `MUL`, `ZXX` and `MIS` are ISO 639-2's
/// undetermined, multiple, no linguistic content and uncoded languages; `MIS` also stands
/// for a code this list does not hold, which `Locale.tag` keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(remote = "klapz_core::language::Language")]
pub enum Language {
    /// Afar.
    Aa,
    /// Abkhazian.
    Ab,
    /// Achinese.
    Ace,
    /// Acoli.
    Ach,
    /// Adangme.
    Ada,
    /// Adyghe.
    Ady,
    /// Avestan.
    Ae,
    /// Tunisian Arabic.
    Aeb,
    /// Afrikaans.
    Af,
    /// Afrihili.
    Afh,
    /// Aghem.
    Agq,
    /// Ainu.
    Ain,
    /// Akan.
    Ak,
    /// Akkadian.
    Akk,
    /// Alabama.
    Akz,
    /// Aleut.
    Ale,
    /// Gheg Albanian.
    Aln,
    /// Southern Altai.
    Alt,
    /// Amharic.
    Am,
    /// Aragonese.
    An,
    /// Old English.
    Ang,
    /// Angika.
    Anp,
    /// Arabic.
    Ar,
    /// Aramaic.
    Arc,
    /// Mapuche.
    Arn,
    /// Araona.
    Aro,
    /// Arapaho.
    Arp,
    /// Algerian Arabic.
    Arq,
    /// Najdi Arabic.
    Ars,
    /// Arawak.
    Arw,
    /// Moroccan Arabic.
    Ary,
    /// Egyptian Arabic.
    Arz,
    /// Assamese.
    As,
    /// Asu.
    Asa,
    /// American Sign Language.
    Ase,
    /// Asturian.
    Ast,
    /// Avaric.
    Av,
    /// Kotava.
    Avk,
    /// Awadhi.
    Awa,
    /// Aymara.
    Ay,
    /// Azerbaijani.
    Az,
    /// Bashkir.
    Ba,
    /// Baluchi.
    Bal,
    /// Balinese.
    Ban,
    /// Bavarian.
    Bar,
    /// Basaa.
    Bas,
    /// Bamun.
    Bax,
    /// Batak Toba.
    Bbc,
    /// Ghomala.
    Bbj,
    /// Belarusian.
    Be,
    /// Beja.
    Bej,
    /// Bemba.
    Bem,
    /// Betawi.
    Bew,
    /// Bena.
    Bez,
    /// Bafut.
    Bfd,
    /// Badaga.
    Bfq,
    /// Bulgarian.
    Bg,
    /// Western Balochi.
    Bgn,
    /// Bhojpuri.
    Bho,
    /// Bislama.
    Bi,
    /// Bikol.
    Bik,
    /// Bini.
    Bin,
    /// Banjar.
    Bjn,
    /// Kom.
    Bkm,
    /// Siksika.
    Bla,
    /// Bambara.
    Bm,
    /// Bangla.
    Bn,
    /// Tibetan.
    Bo,
    /// Bishnupriya.
    Bpy,
    /// Bakhtiari.
    Bqi,
    /// Breton.
    Br,
    /// Braj.
    Bra,
    /// Brahui.
    Brh,
    /// Bodo.
    Brx,
    /// Bosnian.
    Bs,
    /// Akoose.
    Bss,
    /// Buriat.
    Bua,
    /// Buginese.
    Bug,
    /// Bulu.
    Bum,
    /// Blin.
    Byn,
    /// Medumba.
    Byv,
    /// Catalan.
    Ca,
    /// Caddo.
    Cad,
    /// Carib.
    Car,
    /// Cayuga.
    Cay,
    /// Atsam.
    Cch,
    /// Chakma.
    Ccp,
    /// Chechen.
    Ce,
    /// Cebuano.
    Ceb,
    /// Chiga.
    Cgg,
    /// Chamorro.
    Ch,
    /// Chibcha.
    Chb,
    /// Chagatai.
    Chg,
    /// Chuukese.
    Chk,
    /// Mari.
    Chm,
    /// Chinook Jargon.
    Chn,
    /// Choctaw.
    Cho,
    /// Chipewyan.
    Chp,
    /// Cherokee.
    Chr,
    /// Cheyenne.
    Chy,
    /// Chickasaw.
    Cic,
    /// Central Kurdish.
    Ckb,
    /// Corsican.
    Co,
    /// Coptic.
    Cop,
    /// Capiznon.
    Cps,
    /// Cree.
    Cr,
    /// Crimean Turkish.
    Crh,
    /// Seselwa Creole French.
    Crs,
    /// Czech.
    Cs,
    /// Kashubian.
    Csb,
    /// Church Slavic.
    Cu,
    /// Chuvash.
    Cv,
    /// Welsh.
    Cy,
    /// Danish.
    Da,
    /// Dakota.
    Dak,
    /// Dargwa.
    Dar,
    /// Taita.
    Dav,
    /// German.
    De,
    /// Delaware.
    Del,
    /// Slave.
    Den,
    /// Dogrib.
    Dgr,
    /// Dinka.
    Din,
    /// Zarma.
    Dje,
    /// Dogri.
    Doi,
    /// Lower Sorbian.
    Dsb,
    /// Central Dusun.
    Dtp,
    /// Duala.
    Dua,
    /// Middle Dutch.
    Dum,
    /// Divehi.
    Dv,
    /// Jola-Fonyi.
    Dyo,
    /// Dyula.
    Dyu,
    /// Dzongkha.
    Dz,
    /// Dazaga.
    Dzg,
    /// Embu.
    Ebu,
    /// Ewe.
    Ee,
    /// Efik.
    Efi,
    /// Emilian.
    Egl,
    /// Ancient Egyptian.
    Egy,
    /// Ekajuk.
    Eka,
    /// Greek.
    El,
    /// Elamite.
    Elx,
    /// English.
    En,
    /// Middle English.
    Enm,
    /// Esperanto.
    Eo,
    /// Spanish.
    Es,
    /// Central Yupik.
    Esu,
    /// Estonian.
    Et,
    /// Basque.
    Eu,
    /// Ewondo.
    Ewo,
    /// Extremaduran.
    Ext,
    /// Persian.
    Fa,
    /// Fang.
    Fan,
    /// Fanti.
    Fat,
    /// Fulah.
    Ff,
    /// Finnish.
    Fi,
    /// Filipino.
    Fil,
    /// Tornedalen Finnish.
    Fit,
    /// Fijian.
    Fj,
    /// Faroese.
    Fo,
    /// Fon.
    Fon,
    /// French.
    Fr,
    /// Cajun French.
    Frc,
    /// Middle French.
    Frm,
    /// Old French.
    Fro,
    /// Arpitan.
    Frp,
    /// Northern Frisian.
    Frr,
    /// Eastern Frisian.
    Frs,
    /// Friulian.
    Fur,
    /// Western Frisian.
    Fy,
    /// Irish.
    Ga,
    /// Ga.
    Gaa,
    /// Gagauz.
    Gag,
    /// Gan Chinese.
    Gan,
    /// Gayo.
    Gay,
    /// Gbaya.
    Gba,
    /// Zoroastrian Dari.
    Gbz,
    /// Scottish Gaelic.
    Gd,
    /// Geez.
    Gez,
    /// Gilbertese.
    Gil,
    /// Galician.
    Gl,
    /// Gilaki.
    Glk,
    /// Middle High German.
    Gmh,
    /// Guarani.
    Gn,
    /// Old High German.
    Goh,
    /// Goan Konkani.
    Gom,
    /// Gondi.
    Gon,
    /// Gorontalo.
    Gor,
    /// Gothic.
    Got,
    /// Grebo.
    Grb,
    /// Ancient Greek.
    Grc,
    /// Swiss German.
    Gsw,
    /// Gujarati.
    Gu,
    /// Wayuu.
    Guc,
    /// Frafra.
    Gur,
    /// Gusii.
    Guz,
    /// Manx.
    Gv,
    /// Gwichʼin.
    Gwi,
    /// Hausa.
    Ha,
    /// Haida.
    Hai,
    /// Hakka Chinese.
    Hak,
    /// Hawaiian.
    Haw,
    /// Hebrew.
    He,
    /// Hindi.
    Hi,
    /// Fiji Hindi.
    Hif,
    /// Hiligaynon.
    Hil,
    /// Hittite.
    Hit,
    /// Hmong.
    Hmn,
    /// Hiri Motu.
    Ho,
    /// Croatian.
    Hr,
    /// Upper Sorbian.
    Hsb,
    /// Xiang Chinese.
    Hsn,
    /// Haitian Creole.
    Ht,
    /// Hungarian.
    Hu,
    /// Hupa.
    Hup,
    /// Armenian.
    Hy,
    /// Herero.
    Hz,
    /// Interlingua.
    Ia,
    /// Iban.
    Iba,
    /// Ibibio.
    Ibb,
    /// Indonesian.
    Id,
    /// Interlingue.
    Ie,
    /// Igbo.
    Ig,
    /// Sichuan Yi.
    Ii,
    /// Inupiaq.
    Ik,
    /// Iloko.
    Ilo,
    /// Ingush.
    Inh,
    /// Ido.
    Io,
    /// Icelandic.
    Is,
    /// Italian.
    It,
    /// Inuktitut.
    Iu,
    /// Ingrian.
    Izh,
    /// Japanese.
    Ja,
    /// Jamaican Creole English.
    Jam,
    /// Lojban.
    Jbo,
    /// Ngomba.
    Jgo,
    /// Machame.
    Jmc,
    /// Judeo-Persian.
    Jpr,
    /// Judeo-Arabic.
    Jrb,
    /// Jutish.
    Jut,
    /// Javanese.
    Jv,
    /// Georgian.
    Ka,
    /// Kara-Kalpak.
    Kaa,
    /// Kabyle.
    Kab,
    /// Kachin.
    Kac,
    /// Jju.
    Kaj,
    /// Kamba.
    Kam,
    /// Kawi.
    Kaw,
    /// Kabardian.
    Kbd,
    /// Kanembu.
    Kbl,
    /// Tyap.
    Kcg,
    /// Makonde.
    Kde,
    /// Kabuverdianu.
    Kea,
    /// Kenyang.
    Ken,
    /// Koro.
    Kfo,
    /// Kongo.
    Kg,
    /// Kaingang.
    Kgp,
    /// Khasi.
    Kha,
    /// Khotanese.
    Kho,
    /// Koyra Chiini.
    Khq,
    /// Khowar.
    Khw,
    /// Kikuyu.
    Ki,
    /// Kirmanjki.
    Kiu,
    /// Kuanyama.
    Kj,
    /// Kazakh.
    Kk,
    /// Kako.
    Kkj,
    /// Kalaallisut.
    Kl,
    /// Kalenjin.
    Kln,
    /// Khmer.
    Km,
    /// Kimbundu.
    Kmb,
    /// Kannada.
    Kn,
    /// Korean.
    Ko,
    /// Komi-Permyak.
    Koi,
    /// Konkani.
    Kok,
    /// Kosraean.
    Kos,
    /// Kpelle.
    Kpe,
    /// Kanuri.
    Kr,
    /// Karachay-Balkar.
    Krc,
    /// Krio.
    Kri,
    /// Kinaray-a.
    Krj,
    /// Karelian.
    Krl,
    /// Kurukh.
    Kru,
    /// Kashmiri.
    Ks,
    /// Shambala.
    Ksb,
    /// Bafia.
    Ksf,
    /// Colognian.
    Ksh,
    /// Kurdish.
    Ku,
    /// Kumyk.
    Kum,
    /// Kutenai.
    Kut,
    /// Komi.
    Kv,
    /// Cornish.
    Kw,
    /// Kyrgyz.
    Ky,
    /// Latin.
    La,
    /// Ladino.
    Lad,
    /// Langi.
    Lag,
    /// Lahnda.
    Lah,
    /// Lamba.
    Lam,
    /// Luxembourgish.
    Lb,
    /// Lezghian.
    Lez,
    /// Lingua Franca Nova.
    Lfn,
    /// Ganda.
    Lg,
    /// Limburgish.
    Li,
    /// Ligurian.
    Lij,
    /// Livonian.
    Liv,
    /// Lakota.
    Lkt,
    /// Lombard.
    Lmo,
    /// Lingala.
    Ln,
    /// Lao.
    Lo,
    /// Mongo.
    Lol,
    /// Louisiana Creole.
    Lou,
    /// Lozi.
    Loz,
    /// Northern Luri.
    Lrc,
    /// Lithuanian.
    Lt,
    /// Latgalian.
    Ltg,
    /// Luba-Katanga.
    Lu,
    /// Luba-Lulua.
    Lua,
    /// Luiseno.
    Lui,
    /// Lunda.
    Lun,
    /// Luo.
    Luo,
    /// Mizo.
    Lus,
    /// Luyia.
    Luy,
    /// Latvian.
    Lv,
    /// Literary Chinese.
    Lzh,
    /// Laz.
    Lzz,
    /// Madurese.
    Mad,
    /// Mafa.
    Maf,
    /// Magahi.
    Mag,
    /// Maithili.
    Mai,
    /// Makasar.
    Mak,
    /// Mandingo.
    Man,
    /// Masai.
    Mas,
    /// Maba.
    Mde,
    /// Moksha.
    Mdf,
    /// Mandar.
    Mdr,
    /// Mende.
    Men,
    /// Meru.
    Mer,
    /// Morisyen.
    Mfe,
    /// Malagasy.
    Mg,
    /// Middle Irish.
    Mga,
    /// Makhuwa-Meetto.
    Mgh,
    /// Metaʼ.
    Mgo,
    /// Marshallese.
    Mh,
    /// Maori.
    Mi,
    /// Mi'kmaq.
    Mic,
    /// Minangkabau.
    Min,
    /// Uncoded languages.
    Mis,
    /// Macedonian.
    Mk,
    /// Malayalam.
    Ml,
    /// Mongolian.
    Mn,
    /// Manchu.
    Mnc,
    /// Manipuri.
    Mni,
    /// Mohawk.
    Moh,
    /// Mossi.
    Mos,
    /// Marathi.
    Mr,
    /// Western Mari.
    Mrj,
    /// Malay.
    Ms,
    /// Maltese.
    Mt,
    /// Mundang.
    Mua,
    /// Multiple languages.
    Mul,
    /// Muscogee.
    Mus,
    /// Mirandese.
    Mwl,
    /// Marwari.
    Mwr,
    /// Mentawai.
    Mwv,
    /// Burmese.
    My,
    /// Myene.
    Mye,
    /// Erzya.
    Myv,
    /// Mazanderani.
    Mzn,
    /// Nauru.
    Na,
    /// Min Nan Chinese.
    Nan,
    /// Neapolitan.
    Nap,
    /// Nama.
    Naq,
    /// Norwegian Bokmål.
    Nb,
    /// North Ndebele.
    Nd,
    /// Low German.
    Nds,
    /// Nepali.
    Ne,
    /// Newari.
    New,
    /// Ndonga.
    Ng,
    /// Nias.
    Nia,
    /// Niuean.
    Niu,
    /// Ao Naga.
    Njo,
    /// Dutch.
    Nl,
    /// Kwasio.
    Nmg,
    /// Norwegian Nynorsk.
    Nn,
    /// Ngiemboon.
    Nnh,
    /// Norwegian.
    No,
    /// Nogai.
    Nog,
    /// Old Norse.
    Non,
    /// Novial.
    Nov,
    /// N’Ko.
    Nqo,
    /// South Ndebele.
    Nr,
    /// Northern Sotho.
    Nso,
    /// Nuer.
    Nus,
    /// Navajo.
    Nv,
    /// Classical Newari.
    Nwc,
    /// Nyanja.
    Ny,
    /// Nyamwezi.
    Nym,
    /// Nyankole.
    Nyn,
    /// Nyoro.
    Nyo,
    /// Nzima.
    Nzi,
    /// Occitan.
    Oc,
    /// Ojibwa.
    Oj,
    /// Oromo.
    Om,
    /// Odia.
    Or,
    /// Ossetic.
    Os,
    /// Osage.
    Osa,
    /// Ottoman Turkish.
    Ota,
    /// Punjabi.
    Pa,
    /// Pangasinan.
    Pag,
    /// Pahlavi.
    Pal,
    /// Pampanga.
    Pam,
    /// Papiamento.
    Pap,
    /// Palauan.
    Pau,
    /// Picard.
    Pcd,
    /// Nigerian Pidgin.
    Pcm,
    /// Pennsylvania German.
    Pdc,
    /// Plautdietsch.
    Pdt,
    /// Old Persian.
    Peo,
    /// Palatine German.
    Pfl,
    /// Phoenician.
    Phn,
    /// Pali.
    Pi,
    /// Polish.
    Pl,
    /// Piedmontese.
    Pms,
    /// Pontic.
    Pnt,
    /// Pohnpeian.
    Pon,
    /// Prussian.
    Prg,
    /// Old Provençal.
    Pro,
    /// Pashto.
    Ps,
    /// Portuguese.
    Pt,
    /// Quechua.
    Qu,
    /// Kʼicheʼ.
    Quc,
    /// Chimborazo Highland Quichua.
    Qug,
    /// Rajasthani.
    Raj,
    /// Rapanui.
    Rap,
    /// Rarotongan.
    Rar,
    /// Romagnol.
    Rgn,
    /// Riffian.
    Rif,
    /// Romansh.
    Rm,
    /// Rundi.
    Rn,
    /// Romanian.
    Ro,
    /// Rombo.
    Rof,
    /// Romany.
    Rom,
    /// Rotuman.
    Rtm,
    /// Russian.
    Ru,
    /// Rusyn.
    Rue,
    /// Roviana.
    Rug,
    /// Aromanian.
    Rup,
    /// Kinyarwanda.
    Rw,
    /// Rwa.
    Rwk,
    /// Sanskrit.
    Sa,
    /// Sandawe.
    Sad,
    /// Sakha.
    Sah,
    /// Samaritan Aramaic.
    Sam,
    /// Samburu.
    Saq,
    /// Sasak.
    Sas,
    /// Santali.
    Sat,
    /// Saurashtra.
    Saz,
    /// Ngambay.
    Sba,
    /// Sangu.
    Sbp,
    /// Sardinian.
    Sc,
    /// Sicilian.
    Scn,
    /// Scots.
    Sco,
    /// Sindhi.
    Sd,
    /// Sassarese Sardinian.
    Sdc,
    /// Southern Kurdish.
    Sdh,
    /// Northern Sami.
    Se,
    /// Seneca.
    See,
    /// Sena.
    Seh,
    /// Seri.
    Sei,
    /// Selkup.
    Sel,
    /// Koyraboro Senni.
    Ses,
    /// Sango.
    Sg,
    /// Old Irish.
    Sga,
    /// Samogitian.
    Sgs,
    /// Serbo-Croatian.
    Sh,
    /// Tachelhit.
    Shi,
    /// Shan.
    Shn,
    /// Chadian Arabic.
    Shu,
    /// Sinhala.
    Si,
    /// Sidamo.
    Sid,
    /// Slovak.
    Sk,
    /// Slovenian.
    Sl,
    /// Lower Silesian.
    Sli,
    /// Selayar.
    Sly,
    /// Samoan.
    Sm,
    /// Southern Sami.
    Sma,
    /// Lule Sami.
    Smj,
    /// Inari Sami.
    Smn,
    /// Skolt Sami.
    Sms,
    /// Shona.
    Sn,
    /// Soninke.
    Snk,
    /// Somali.
    So,
    /// Sogdien.
    Sog,
    /// Albanian.
    Sq,
    /// Serbian.
    Sr,
    /// Sranan Tongo.
    Srn,
    /// Serer.
    Srr,
    /// Swati.
    Ss,
    /// Saho.
    Ssy,
    /// Southern Sotho.
    St,
    /// Saterland Frisian.
    Stq,
    /// Sundanese.
    Su,
    /// Sukuma.
    Suk,
    /// Susu.
    Sus,
    /// Sumerian.
    Sux,
    /// Swedish.
    Sv,
    /// Swahili.
    Sw,
    /// Comorian.
    Swb,
    /// Classical Syriac.
    Syc,
    /// Syriac.
    Syr,
    /// Silesian.
    Szl,
    /// Tamil.
    Ta,
    /// Tulu.
    Tcy,
    /// Telugu.
    Te,
    /// Timne.
    Tem,
    /// Teso.
    Teo,
    /// Tereno.
    Ter,
    /// Tetum.
    Tet,
    /// Tajik.
    Tg,
    /// Thai.
    Th,
    /// Tigrinya.
    Ti,
    /// Tigre.
    Tig,
    /// Tiv.
    Tiv,
    /// Turkmen.
    Tk,
    /// Tokelau.
    Tkl,
    /// Tsakhur.
    Tkr,
    /// Tagalog.
    Tl,
    /// Klingon.
    Tlh,
    /// Tlingit.
    Tli,
    /// Talysh.
    Tly,
    /// Tamashek.
    Tmh,
    /// Tswana.
    Tn,
    /// Tongan.
    To,
    /// Nyasa Tonga.
    Tog,
    /// Tok Pisin.
    Tpi,
    /// Turkish.
    Tr,
    /// Turoyo.
    Tru,
    /// Taroko.
    Trv,
    /// Tsonga.
    Ts,
    /// Tsakonian.
    Tsd,
    /// Tsimshian.
    Tsi,
    /// Tatar.
    Tt,
    /// Muslim Tat.
    Ttt,
    /// Tumbuka.
    Tum,
    /// Tuvalu.
    Tvl,
    /// Twi.
    Tw,
    /// Tasawaq.
    Twq,
    /// Tahitian.
    Ty,
    /// Tuvinian.
    Tyv,
    /// Central Atlas Tamazight.
    Tzm,
    /// Udmurt.
    Udm,
    /// Uyghur.
    Ug,
    /// Ugaritic.
    Uga,
    /// Ukrainian.
    Uk,
    /// Umbundu.
    Umb,
    /// Unknown language.
    Und,
    /// Urdu.
    Ur,
    /// Uzbek.
    Uz,
    /// Vai.
    Vai,
    /// Venda.
    Ve,
    /// Venetian.
    Vec,
    /// Veps.
    Vep,
    /// Vietnamese.
    Vi,
    /// West Flemish.
    Vls,
    /// Main-Franconian.
    Vmf,
    /// Volapük.
    Vo,
    /// Votic.
    Vot,
    /// Võro.
    Vro,
    /// Vunjo.
    Vun,
    /// Walloon.
    Wa,
    /// Walser.
    Wae,
    /// Wolaytta.
    Wal,
    /// Waray.
    War,
    /// Washo.
    Was,
    /// Warlpiri.
    Wbp,
    /// Wolof.
    Wo,
    /// Wu Chinese.
    Wuu,
    /// Kalmyk.
    Xal,
    /// Xhosa.
    Xh,
    /// Mingrelian.
    Xmf,
    /// Soga.
    Xog,
    /// Yao.
    Yao,
    /// Yapese.
    Yap,
    /// Yangben.
    Yav,
    /// Yemba.
    Ybb,
    /// Yiddish.
    Yi,
    /// Yoruba.
    Yo,
    /// Nheengatu.
    Yrl,
    /// Cantonese.
    Yue,
    /// Zhuang.
    Za,
    /// Zapotec.
    Zap,
    /// Blissymbols.
    Zbl,
    /// Zeelandic.
    Zea,
    /// Zenaga.
    Zen,
    /// Standard Moroccan Tamazight.
    Zgh,
    /// Chinese.
    Zh,
    /// Zulu.
    Zu,
    /// Zuni.
    Zun,
    /// No linguistic content.
    Zxx,
    /// Zaza.
    Zza,
}
