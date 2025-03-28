pub fn validate_constellation(constellation: &str) -> bool {
    match constellation.to_lowercase().as_str() {
        "aries" | "taurus" | "gemini" | "cancer" | "leo" | "virgo" | "libra" | "scorpio"
        | "sagittarius" | "capricorn" | "aquarius" | "pisces" => true,
        _ => false,
    }
}
