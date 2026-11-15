//! Names for `Intl.DisplayNames`: what Chrome 157 answers in English for
//! every code it has a name for. Each table is `name=code code|…`, and is
//! unpacked once per process, when first asked.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Regions: ISO 3166 codes, the ones CLDR adds (`EU`, `XK`, `ZZ`), and
/// `419`, the only UN area code Chrome has a name for.
const REGION: &str = "\
    Latin America=419|Ascension Island=AC|Andorra=AD|United Arab Emirates=AE|Afghanistan=AF|\
    Antigua & Barbuda=AG|Anguilla=AI|Albania=AL|Armenia=AM|Curaçao=AN CW|Angola=AO|\
    Antarctica=AQ|Argentina=AR|American Samoa=AS|Austria=AT|Australia=AU|Aruba=AW|\
    Åland Islands=AX|Azerbaijan=AZ|Bosnia & Herzegovina=BA|Barbados=BB|Bangladesh=BD|Belgium=BE|\
    Burkina Faso=BF HV|Bulgaria=BG|Bahrain=BH|Burundi=BI|Benin=BJ DY|St. Barthélemy=BL|\
    Bermuda=BM|Brunei=BN|Bolivia=BO|Caribbean Netherlands=BQ|Brazil=BR|Bahamas=BS|Bhutan=BT|\
    Myanmar (Burma)=BU MM|Bouvet Island=BV|Botswana=BW|Belarus=BY|Belize=BZ|Canada=CA|\
    Cocos (Keeling) Islands=CC|Congo - Kinshasa=CD ZR|Central African Republic=CF|\
    Congo - Brazzaville=CG|Switzerland=CH|Côte d’Ivoire=CI|Cook Islands=CK|Chile=CL|Cameroon=CM|\
    China=CN|Colombia=CO|Clipperton Island=CP|Sark=CQ|Costa Rica=CR|Serbia=CS RS YU|Cuba=CU|\
    Cape Verde=CV|Christmas Island=CX|Cyprus=CY|Czechia=CZ|Germany=DD DE|Diego Garcia=DG|\
    Djibouti=DJ|Denmark=DK|Dominica=DM|Dominican Republic=DO|Algeria=DZ|Ceuta & Melilla=EA|\
    Ecuador=EC|Estonia=EE|Egypt=EG|Western Sahara=EH|Eritrea=ER|Spain=ES|Ethiopia=ET|\
    European Union=EU|Eurozone=EZ|Finland=FI|Fiji=FJ|Falkland Islands (Islas Malvinas)=FK|\
    Micronesia=FM|Faroe Islands=FO|France=FR FX|Gabon=GA|United Kingdom=GB UK|Grenada=GD|\
    Georgia=GE|French Guiana=GF|Guernsey=GG|Ghana=GH|Gibraltar=GI|Greenland=GL|Gambia=GM|\
    Guinea=GN|Guadeloupe=GP|Equatorial Guinea=GQ|Greece=GR|\
    South Georgia & South Sandwich Islands=GS|Guatemala=GT|Guam=GU|Guinea-Bissau=GW|Guyana=GY|\
    Hong Kong=HK|Heard & McDonald Islands=HM|Honduras=HN|Croatia=HR|Haiti=HT|Hungary=HU|\
    Canary Islands=IC|Indonesia=ID|Ireland=IE|Israel=IL|Isle of Man=IM|India=IN|\
    British Indian Ocean Territory=IO|Iraq=IQ|Iran=IR|Iceland=IS|Italy=IT|Jersey=JE|Jamaica=JM|\
    Jordan=JO|Japan=JP|Kenya=KE|Kyrgyzstan=KG|Cambodia=KH|Kiribati=KI|Comoros=KM|\
    St. Kitts & Nevis=KN|North Korea=KP|South Korea=KR|Kuwait=KW|Cayman Islands=KY|\
    Kazakhstan=KZ|Laos=LA|Lebanon=LB|St. Lucia=LC|Liechtenstein=LI|Sri Lanka=LK|Liberia=LR|\
    Lesotho=LS|Lithuania=LT|Luxembourg=LU|Latvia=LV|Libya=LY|Morocco=MA|Monaco=MC|Moldova=MD|\
    Montenegro=ME|St. Martin=MF|Madagascar=MG|Marshall Islands=MH|North Macedonia=MK|Mali=ML|\
    Mongolia=MN|Macao=MO|Northern Mariana Islands=MP|Martinique=MQ|Mauritania=MR|Montserrat=MS|\
    Malta=MT|Mauritius=MU|Maldives=MV|Malawi=MW|Mexico=MX|Malaysia=MY|Mozambique=MZ|Namibia=NA|\
    New Caledonia=NC|Niger=NE|Norfolk Island=NF|Nigeria=NG|Vanuatu=NH VU|Nicaragua=NI|\
    Netherlands=NL|Norway=NO|Nepal=NP|Nauru=NR|Niue=NU|New Zealand=NZ|Oman=OM|Panama=PA|Peru=PE|\
    French Polynesia=PF|Papua New Guinea=PG|Philippines=PH|Pakistan=PK|Poland=PL|\
    St. Pierre & Miquelon=PM|Pitcairn Islands=PN|Puerto Rico=PR|Palestine=PS|Portugal=PT|\
    Palau=PW|Paraguay=PY|Qatar=QA|Outlying Oceania=QO|Réunion=RE|Zimbabwe=RH ZW|Romania=RO|\
    Russia=RU SU|Rwanda=RW|Saudi Arabia=SA|Solomon Islands=SB|Seychelles=SC|Sudan=SD|Sweden=SE|\
    Singapore=SG|St. Helena=SH|Slovenia=SI|Svalbard & Jan Mayen=SJ|Slovakia=SK|Sierra Leone=SL|\
    San Marino=SM|Senegal=SN|Somalia=SO|Suriname=SR|South Sudan=SS|São Tomé & Príncipe=ST|\
    El Salvador=SV|Sint Maarten=SX|Syria=SY|Eswatini=SZ|Tristan da Cunha=TA|\
    Turks & Caicos Islands=TC|Chad=TD|French Southern Territories=TF|Togo=TG|Thailand=TH|\
    Tajikistan=TJ|Tokelau=TK|Timor-Leste=TL TP|Turkmenistan=TM|Tunisia=TN|Tonga=TO|Türkiye=TR|\
    Trinidad & Tobago=TT|Tuvalu=TV|Taiwan=TW|Tanzania=TZ|Ukraine=UA|Uganda=UG|\
    U.S. Outlying Islands=UM|United Nations=UN|United States=US|Uruguay=UY|Uzbekistan=UZ|\
    Vatican City=VA|St. Vincent & Grenadines=VC|Vietnam=VD VN|Venezuela=VE|\
    British Virgin Islands=VG|U.S. Virgin Islands=VI|Wallis & Futuna=WF|Samoa=WS|\
    Pseudo-Accents=XA|Pseudo-Bidi=XB|Kosovo=XK|Yemen=YD YE|Mayotte=YT|South Africa=ZA|Zambia=ZM|\
    Unknown Region=ZZ";

/// Regions whose short and narrow names differ.
const REGION_SHORT: &str = "\
    Bosnia=BA|Cocos Islands=CC|UK=GB|Myanmar=MM|Pitcairn=PN|UN=UN|US=US";

/// Languages, with the three-letter codes that name the same ones.
const LANGUAGE: &str = "\
    Afrikaans=af afr|Akan=ak aka fat tw twi|Albanian=alb als sq sqi|Amharic=am amh|\
    Aragonese=an arg|Arabic=ar ara arb|Armenian=arm hy hye|Assamese=as asm|Asturian=ast|\
    Aymara=ay aym ayr|Azerbaijani=az aze azj|Bambara=bam bm|Basque=baq eu eus|Belarusian=be bel|\
    Bangla=ben bn|Bulgarian=bg bul|Bhojpuri=bh bho bih|Bosnian=bos bs|Breton=br bre|\
    Burmese=bur my mya|Catalan=ca cat|Cebuano=ceb|Czech=ces cs cze|Chinese=chi cmn zh zho|\
    Cherokee=chr|Central Kurdish=ckb|Sanskrit=cls sa san|Montenegrin=cnr|Corsican=co cos|\
    Welsh=cy cym wel|Danish=da dan|German=de deu ger|Dogri=dgo doi|Divehi=div dv|\
    Mongolian=drh khk mn mon|Dari=drw prs tnf|Dutch=dut nl nld|Ewe=ee ewe|Estonian=ekk est et|\
    Greek=el ell gre|English=en eng|Esperanto=eo epo|Spanish=es spa|Persian=fa fas per pes|\
    Faroese=fao fo|Finnish=fi fin|Filipino=fil tgl tl|French=fr fra fre|Western Frisian=fry fy|\
    Irish=ga gle|Oromo=gaz om orm|Scottish Gaelic=gd gla|Georgian=geo ka kat|Galician=gl glg|\
    Guarani=gn grn gug|Konkani=gom kok|Gujarati=gu guj prp|Hausa=ha hau|Haitian Creole=hat ht|\
    Hawaiian=haw|Serbian (Latin)=hbs sh|Hebrew=he heb iw|Hmong=hea hmn|Hindi=hi hin|\
    Croatian=hr hrv scr|Hungarian=hu hun|Interlingua=ia ina|Igbo=ibo ig|Icelandic=ice is isl|\
    Indonesian=id in ind|Iloko=ilo|Italian=it ita|Japanese=ja jpn|Javanese=jav jaw jv jw|\
    Yiddish=ji ydd yi yid|Kannada=kan kn|Kazakh=kaz kk|Khmer=khm km|Kinyarwanda=kin rw|\
    Kyrgyz=kir ky|Kurdish=kmr ku kur|Korean=ko kor|Krio=kri|Latin=la lat|Lao=lao lo|\
    Latvian=lav lv lvs|Luxembourgish=lb ltz|Ganda=lg lug|Lingala=lin ln|Lithuanian=lit lt|\
    Mizo=lus|Macedonian=mac mk mkd|Maithili=mai|Malayalam=mal ml|Māori=mao mi mri|\
    Marathi=mar mr|Malay=may ms msa zsm|Malagasy=mg mlg plt|Maltese=mlt mt|Manipuri=mni|\
    Romanian=mo mol ro ron rum|Norwegian Bokmål=nb nob|Nepali=ne nep npi|\
    Norwegian Nynorsk=nn nno|Norwegian=no nor|Northern Sotho=nso|Nyanja=ny nya|Occitan=oc oci|\
    Odia=or ori ory|Punjabi=pa pan|Pashto=pbu ps pus|Polish=pl pol|Portuguese=por pt|\
    Quechua=qu que quz|Romansh=rm roh|Russian=ru rus|Serbian=scc sr srp|Sindhi=sd snd|\
    Sinhala=si sin|Slovak=sk slk slo|Slovenian=sl slv|Samoan=sm smo|Shona=sn sna|Somali=so som|\
    Southern Sotho=sot st|Sundanese=su sun|Swedish=sv swe|Swahili=sw swa swh|Congo Swahili=swc|\
    Tamil=ta tam|Tatar=tat tt|Telugu=te tel|Tajik=tg tgk|Thai=th tha|Tigrinya=ti tir|\
    Turkmen=tk tuk|Tswana=tn tsn|Tongan=to ton|Turkish=tr tur|Tsonga=ts tso|Uyghur=ug uig|\
    Ukrainian=uk ukr|Urdu=ur urd|Uzbek=uz uzb uzn|Vietnamese=vi vie|Walloon=wa wln|Wolof=wo wol|\
    Xhosa=xh xho|Yoruba=yo yor|Cantonese=yue|Zulu=zu zul";

/// Languages whose short and narrow names differ.
const LANGUAGE_SHORT: &str = "\
    Azeri=az aze azj";

/// Languages as written or spoken in one place, which have names of their
/// own: by the language's name, then the script or region.
const DIALECT: &str = "\
    Modern Standard Arabic=Arabic-001|Simplified Chinese=Chinese-Hans|\
    Traditional Chinese=Chinese-Hant|Flemish=Dutch-BE|Australian English=English-AU|\
    Canadian English=English-CA|British English=English-GB English-UK|\
    American English=English-US|Canadian French=French-CA|Swiss French=French-CH|\
    Austrian German=German-AT|Swiss High German=German-CH|Dari=Persian-AF|\
    Brazilian Portuguese=Portuguese-BR|European Portuguese=Portuguese-PT|Moldavian=Romanian-MD|\
    Montenegrin=Serbian-ME|Latin American Spanish=Spanish-419|European Spanish=Spanish-ES|\
    Mexican Spanish=Spanish-MX|Congo Swahili=Swahili-CD Swahili-ZR";

/// Scripts: ISO 15924 codes.
const SCRIPT: &str = "\
    Adlam=Adlm|Afaka=Afak|Caucasian Albanian=Aghb|Ahom=Ahom|Arabic=Arab|Nastaliq=Aran|\
    Imperial Aramaic=Armi|Armenian=Armn|Avestan=Avst|Balinese=Bali|Bamum=Bamu|Bassa Vah=Bass|\
    Batak=Batk|Bangla=Beng|Beria Erfe=Berf|Bhaiksuki=Bhks|Blissymbols=Blis|Bopomofo=Bopo|\
    Brahmi=Brah|Braille=Brai|Buginese=Bugi|Buhid=Buhd|Chakma=Cakm|\
    Unified Canadian Aboriginal Syllabics=Cans|Carian=Cari|Cham=Cham|Cherokee=Cher|\
    Chorasmian=Chrs|Cirth=Cirt|Coptic=Copt|Cypro-Minoan=Cpmn|Cypriot=Cprt|Cyrillic=Cyrl|\
    Old Church Slavonic Cyrillic=Cyrs|Devanagari=Deva|Dives Akuru=Diak|Dogra=Dogr|Deseret=Dsrt|\
    Duployan shorthand=Dupl|Egyptian demotic=Egyd|Egyptian hieratic=Egyh|\
    Egyptian hieroglyphs=Egyp|Elbasan=Elba|Elymaic=Elym|Ethiopic=Ethi|Garay=Gara|\
    Georgian Khutsuri=Geok|Georgian=Geor|Glagolitic=Glag|Gunjala Gondi=Gong|Masaram Gondi=Gonm|\
    Gothic=Goth|Grantha=Gran|Greek=Grek|Gujarati=Gujr|Gurung Khema=Gukh|Gurmukhi=Guru|\
    Han with Bopomofo=Hanb|Hangul=Hang|Han=Hani|Hanunoo=Hano|Simplified=Hans|Traditional=Hant|\
    Hatran=Hatr|Hebrew=Hebr|Hiragana=Hira|Anatolian Hieroglyphs=Hluw|Pahawh Hmong=Hmng|\
    Nyiakeng Puachue Hmong=Hmnp|Japanese syllabaries=Hrkt|Old Hungarian=Hung|Indus=Inds|\
    Old Italic=Ital|Jamo=Jamo|Javanese=Java|Japanese=Jpan|Jurchen=Jurc|Kayah Li=Kali|\
    Katakana=Kana|Kawi=Kawi|Kharoshthi=Khar|Khmer=Khmr|Khojki=Khoj|Khitan small script=Kits|\
    Kannada=Knda|Korean=Kore|Kpelle=Kpel|Kirat Rai=Krai|Kaithi=Kthi|Lanna=Lana|Lao=Laoo|\
    Fraktur Latin=Latf|Gaelic Latin=Latg|Latin=Latn|Lepcha=Lepc|Limbu=Limb|Linear A=Lina|\
    Linear B=Linb|Fraser=Lisu|Loma=Loma|Lycian=Lyci|Lydian=Lydi|Mahajani=Mahj|Makasar=Maka|\
    Mandaean=Mand|Manichaean=Mani|Marchen=Marc|Mayan hieroglyphs=Maya|Medefaidrin=Medf|\
    Mende=Mend|Meroitic Cursive=Merc|Meroitic=Mero|Malayalam=Mlym|Modi=Modi|Mongolian=Mong|\
    Moon=Moon|Mro=Mroo|Meitei Mayek=Mtei|Multani=Mult|Myanmar=Mymr|Nag Mundari=Nagm|\
    Nandinagari=Nand|Old North Arabian=Narb|Nabataean=Nbat|Newa=Newa|Naxi Geba=Nkgb|N’Ko=Nkoo|\
    Nüshu=Nshu|Ogham=Ogam|Ol Chiki=Olck|Ol Onal=Onao|Orkhon=Orkh|Odia=Orya|Osage=Osge|\
    Osmanya=Osma|Old Uyghur=Ougr|Palmyrene=Palm|Pau Cin Hau=Pauc|Old Permic=Perm|Phags-pa=Phag|\
    Inscriptional Pahlavi=Phli|Psalter Pahlavi=Phlp|Book Pahlavi=Phlv|Phoenician=Phnx|\
    Pollard Phonetic=Plrd|Inscriptional Parthian=Prti|Zawgyi=Qaag|Rejang=Rjng|Hanifi=Rohg|\
    Rongorongo=Roro|Runic=Runr|Samaritan=Samr|Sarati=Sara|Old South Arabian=Sarb|\
    Saurashtra=Saur|SignWriting=Sgnw|Shavian=Shaw|Sharada=Shrd|Siddham=Sidd|Sidetic=Sidt|\
    Khudawadi=Sind|Sinhala=Sinh|Sogdian=Sogd|Old Sogdian=Sogo|Sora Sompeng=Sora|Soyombo=Soyo|\
    Sundanese=Sund|Sunuwar=Sunu|Syloti Nagri=Sylo|Syriac=Syrc|Estrangelo Syriac=Syre|\
    Western Syriac=Syrj|Eastern Syriac=Syrn|Tagbanwa=Tagb|Takri=Takr|Tai Le=Tale|\
    New Tai Lue=Talu|Tamil=Taml|Tangut=Tang|Tai Viet=Tavt|Tai Yo=Tayo|Telugu=Telu|Tengwar=Teng|\
    Tifinagh=Tfng|Tagalog=Tglg|Thaana=Thaa|Thai=Thai|Tibetan=Tibt|Tirhuta=Tirh|Tangsa=Tnsa|\
    Todhri=Todr|Tolong Siki=Tols|Toto=Toto|Tulu-Tigalari=Tutg|Ugaritic=Ugar|Vai=Vaii|\
    Visible Speech=Visp|Vithkuqi=Vith|Varang Kshiti=Wara|Wancho=Wcho|Woleai=Wole|\
    Old Persian=Xpeo|Sumero-Akkadian Cuneiform=Xsux|Yezidi=Yezi|Yi=Yiii|Zanabazar Square=Zanb|\
    Inherited=Zinh|Mathematical Notation=Zmth|Emoji=Zsye|Symbols=Zsym|Unwritten=Zxxx|\
    Common=Zyyy|Unknown Script=Zzzz";

/// Scripts whose short and narrow names differ.
const SCRIPT_SHORT: &str = "\
    UCAS=Cans|S-A Cuneiform=Xsux";

/// Currencies: ISO 4217 codes.
const CURRENCY: &str = "\
    United Arab Emirates Dirham=AED|Afghan Afghani=AFN|Albanian Lek=ALL|Armenian Dram=AMD|\
    Netherlands Antillean Guilder=ANG|Angolan Kwanza=AOA|Argentine Peso=ARS|\
    Australian Dollar=AUD|Aruban Florin=AWG|Azerbaijani Manat=AZN|\
    Bosnia-Herzegovina Convertible Mark=BAM|Barbadian Dollar=BBD|Bangladeshi Taka=BDT|\
    Bulgarian Lev=BGN|Bahraini Dinar=BHD|Burundian Franc=BIF|Bermudan Dollar=BMD|\
    Brunei Dollar=BND|Bolivian Boliviano=BOB|Brazilian Real=BRL|Bahamian Dollar=BSD|\
    Bhutanese Ngultrum=BTN|Botswanan Pula=BWP|Belarusian Ruble=BYN|Belize Dollar=BZD|\
    Canadian Dollar=CAD|Congolese Franc=CDF|Swiss Franc=CHF|Chilean Peso=CLP|Chinese Yuan=CNY|\
    Colombian Peso=COP|Costa Rican Colón=CRC|Cuban Convertible Peso=CUC|Cuban Peso=CUP|\
    Cape Verdean Escudo=CVE|Czech Koruna=CZK|Djiboutian Franc=DJF|Danish Krone=DKK|\
    Dominican Peso=DOP|Algerian Dinar=DZD|Egyptian Pound=EGP|Eritrean Nakfa=ERN|\
    Ethiopian Birr=ETB|Euro=EUR|Fijian Dollar=FJD|Falkland Islands Pound=FKP|British Pound=GBP|\
    Georgian Lari=GEL|Ghanaian Cedi=GHS|Gibraltar Pound=GIP|Gambian Dalasi=GMD|\
    Guinean Franc=GNF|Guatemalan Quetzal=GTQ|Guyanaese Dollar=GYD|Hong Kong Dollar=HKD|\
    Honduran Lempira=HNL|Croatian Kuna=HRK|Haitian Gourde=HTG|Hungarian Forint=HUF|\
    Indonesian Rupiah=IDR|Israeli New Shekel=ILS|Indian Rupee=INR|Iraqi Dinar=IQD|\
    Iranian Rial=IRR|Icelandic Króna=ISK|Jamaican Dollar=JMD|Jordanian Dinar=JOD|\
    Japanese Yen=JPY|Kenyan Shilling=KES|Kyrgyz Som=KGS|Cambodian Riel=KHR|Comorian Franc=KMF|\
    North Korean Won=KPW|South Korean Won=KRW|Kuwaiti Dinar=KWD|Cayman Islands Dollar=KYD|\
    Kazakhstani Tenge=KZT|Laotian Kip=LAK|Lebanese Pound=LBP|Sri Lankan Rupee=LKR|\
    Liberian Dollar=LRD|Lesotho Loti=LSL|Libyan Dinar=LYD|Moroccan Dirham=MAD|Moldovan Leu=MDL|\
    Malagasy Ariary=MGA|Macedonian Denar=MKD|Myanmar Kyat=MMK|Mongolian Tugrik=MNT|\
    Macanese Pataca=MOP|Mauritanian Ouguiya=MRU|Mauritian Rupee=MUR|Maldivian Rufiyaa=MVR|\
    Malawian Kwacha=MWK|Mexican Peso=MXN|Malaysian Ringgit=MYR|Mozambican Metical=MZN|\
    Namibian Dollar=NAD|Nigerian Naira=NGN|Nicaraguan Córdoba=NIO|Norwegian Krone=NOK|\
    Nepalese Rupee=NPR|New Zealand Dollar=NZD|Omani Rial=OMR|Panamanian Balboa=PAB|\
    Peruvian Sol=PEN|Papua New Guinean Kina=PGK|Philippine Peso=PHP|Pakistani Rupee=PKR|\
    Polish Zloty=PLN|Paraguayan Guarani=PYG|Qatari Riyal=QAR|Romanian Leu=RON|Serbian Dinar=RSD|\
    Russian Ruble=RUB|Rwandan Franc=RWF|Saudi Riyal=SAR|Solomon Islands Dollar=SBD|\
    Seychellois Rupee=SCR|Sudanese Pound=SDG|Swedish Krona=SEK|Singapore Dollar=SGD|\
    St. Helena Pound=SHP|Sierra Leonean Leone (1964—2022)=SLL|Somali Shilling=SOS|\
    Surinamese Dollar=SRD|South Sudanese Pound=SSP|São Tomé & Príncipe Dobra=STN|\
    Salvadoran Colón=SVC|Syrian Pound=SYP|Swazi Lilangeni=SZL|Thai Baht=THB|\
    Tajikistani Somoni=TJS|Turkmenistani Manat=TMT|Tunisian Dinar=TND|Tongan Paʻanga=TOP|\
    Turkish Lira=TRY|Trinidad & Tobago Dollar=TTD|New Taiwan Dollar=TWD|Tanzanian Shilling=TZS|\
    Ukrainian Hryvnia=UAH|Ugandan Shilling=UGX|US Dollar=USD|Uruguayan Peso=UYU|\
    Uzbekistani Som=UZS|Venezuelan Bolívar=VES|Vietnamese Dong=VND|Vanuatu Vatu=VUV|\
    Samoan Tala=WST|Central African CFA Franc=XAF|East Caribbean Dollar=XCD|\
    Special Drawing Rights=XDR|West African CFA Franc=XOF|CFP Franc=XPF|Sucre=XSU|\
    Yemeni Rial=YER|South African Rand=ZAR|Zambian Kwacha=ZMW|Zimbabwean Dollar (2009–2024)=ZWL";

/// Calendars.
const CALENDAR: &str = "\
    Buddhist Calendar=buddhist|Chinese Calendar=chinese|Coptic Calendar=coptic|\
    Dangi Calendar=dangi|Ethiopic Amete Alem Calendar=ethioaa ethiopic-amete-alem|\
    Ethiopic Calendar=ethiopic|Gregorian Calendar=gregory|Hebrew Calendar=hebrew|\
    Indian National Calendar=indian|Hijri Calendar=islamic|\
    Hijri Calendar (tabular, civil epoch)=islamic-civil|\
    Hijri Calendar (Saudi Arabia, sighting)=islamic-rgsa|\
    Hijri Calendar (tabular, astronomical epoch)=islamic-tbla|\
    Hijri Calendar (Umm al-Qura)=islamic-umalqura|Gregorian Calendar (ISO 8601 Weeks)=iso8601|\
    Japanese Calendar=japanese|Persian Calendar=persian|Minguo Calendar=roc";

const TABLES: [(&str, &str); 9] = [
    ("region", REGION),
    ("regionShort", REGION_SHORT),
    ("language", LANGUAGE),
    ("languageShort", LANGUAGE_SHORT),
    ("dialect", DIALECT),
    ("script", SCRIPT),
    ("scriptShort", SCRIPT_SHORT),
    ("currency", CURRENCY),
    ("calendar", CALENDAR),
];

type Names = HashMap<&'static str, &'static str>;

/// The name `table` has for `code`, exactly as written.
pub(super) fn name(table: &str, code: &str) -> Option<&'static str> {
    static UNPACKED: [OnceLock<Names>; TABLES.len()] = [const { OnceLock::new() }; TABLES.len()];
    let at = TABLES.iter().position(|t| t.0 == table)?;
    UNPACKED[at]
        .get_or_init(|| {
            let mut names = Names::new();
            for row in TABLES[at].1.split('|') {
                let (name, codes) = row.split_once('=').expect("a name and its codes");
                for code in codes.split(' ') {
                    names.insert(code, name);
                }
            }
            names
        })
        .get(code)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_unpacks() {
        for (table, packed) in TABLES {
            let codes = packed.split('|').flat_map(|row| {
                let (name, codes) = row.split_once('=').unwrap();
                codes.split(' ').map(move |code| (code, name))
            });
            let mut count = 0;
            for (code, expected) in codes {
                assert!(
                    !code.is_empty() && !expected.is_empty(),
                    "{table}: {code:?}"
                );
                assert_eq!(name(table, code), Some(expected), "{table}: {code} twice");
                count += 1;
            }
            assert!(count > 0, "{table}");
        }
        assert_eq!(name("region", "TR"), Some("Türkiye"));
        assert_eq!(name("region", "tr"), None);
        assert_eq!(name("regionShort", "GB"), Some("UK"));
        assert_eq!(name("dialect", "English-US"), Some("American English"));
        assert_eq!(name("planet", "Mars"), None);
    }
}
