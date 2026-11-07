use super::*;

const REAL_CLUSTERS: &[(&str, &str, usize)] = &[
    ("ascii", "a", 1),
    ("space", " ", 1),
    ("cjk wide", "\u{6C34}", 3),
    ("combining acute", "e\u{0301}", 3),
    ("box drawing", "\u{2502}", 3),
    ("regional flag", "\u{1F1FA}\u{1F1F8}", 8),
    ("skin tone", "\u{1F44D}\u{1F3FD}", 8),
    ("devanagari conjunct", "\u{915}\u{94D}\u{937}\u{93F}", 12),
    ("zwj + vs16 flag", "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}", 14),
    (
        "zwj family of 3",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        18,
    ),
    (
        "zwj family of 4",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
        25,
    ),
    (
        "tag sequence flag",
        "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}",
        28,
    ),
];

#[test]
fn every_real_cluster_round_trips_byte_exactly() {
    for (name, cluster, expected_len) in REAL_CLUSTERS {
        assert_eq!(
            cluster.len(),
            *expected_len,
            "{name}: the test table's own byte count is wrong"
        );
        let cell = InlineGrapheme::new(cluster);
        assert_eq!(
            cell.as_str(),
            *cluster,
            "{name} ({expected_len} bytes) must round-trip byte-exactly"
        );
        assert_eq!(cell.len(), *expected_len, "{name}: len must be the input's");
    }
}
