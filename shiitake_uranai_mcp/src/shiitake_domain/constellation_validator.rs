pub fn validate_constellation(constellation: &str) -> bool {
    matches!(
        constellation,
        "aries"
            | "taurus"
            | "gemini"
            | "cancer"
            | "leo"
            | "virgo"
            | "libra"
            | "scorpio"
            | "sagittarius"
            | "capricorn"
            | "aquarius"
            | "pisces"
    )
}
