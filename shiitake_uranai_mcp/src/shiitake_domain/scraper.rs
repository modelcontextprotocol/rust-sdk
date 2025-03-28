use chrono::Datelike;
use chrono::{Duration, TimeZone, Utc};
use chrono_tz::Asia::Tokyo;
use reqwest::Client;
use scraper::{Html, Selector};

fn get_latest_monday_date() -> String {
    let utc = Utc::now().naive_utc();
    let jst_now = Tokyo.from_utc_datetime(&utc);
    let weekday = jst_now.weekday().num_days_from_monday();
    let days_to_subtract = if weekday == 0 { 0 } else { weekday };
    let last_monday = jst_now.date_naive() - Duration::days(days_to_subtract as i64);
    let formatted_date = last_monday.format("%Y-%m-%d").to_string();
    formatted_date
}

pub async fn scrape(constellation: String) -> Result<String, Box<dyn std::error::Error>> {
    let formatted_date = get_latest_monday_date();
    let url = format!(
        "https://shiitakeuranai.jp/weekly-horoscope/{}/{}",
        formatted_date, constellation
    );

    let client = Client::new();
    let response = client.get(&url).send().await?.text().await?;

    let document = Html::parse_document(&response);
    let mut fortune_text = String::new();

    // 最初のセクションの文章を取得
    let selector1 = Selector::parse(
        "body > main > section:nth-child(1) > div > div._root_hvc32_1._content_vbk6l_74 > p",
    )
    .unwrap();
    for element in document.select(&selector1) {
        fortune_text.push_str(&element.text().collect::<Vec<_>>().join(""));
    }

    // 二番目のセクションの文章を取得
    let selector2 = Selector::parse("body > main > section:nth-child(3) > div > div > p").unwrap();
    for element in document.select(&selector2) {
        fortune_text.push_str(&element.text().collect::<Vec<_>>().join(""));
    }

    Ok(fortune_text)
}
