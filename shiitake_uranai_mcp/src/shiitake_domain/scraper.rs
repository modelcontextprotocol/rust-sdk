use chrono::Datelike;
use chrono::{Duration, TimeZone, Utc};
use chrono_tz::Asia::Tokyo;
use reqwest::Client;
use scraper::{Html, Selector};

fn get_monday_date(weeks_back: i64) -> String {
    let utc = Utc::now().naive_utc();
    let jst_now = Tokyo.from_utc_datetime(&utc);
    let weekday = jst_now.weekday().num_days_from_monday();
    let days_to_subtract = if weekday == 0 { 0 } else { weekday } as i64;

    // 指定された週数分、さらに過去に戻る
    let target_monday = jst_now.date_naive() - Duration::days(days_to_subtract + (weeks_back * 7));
    let formatted_date = target_monday.format("%Y-%m-%d").to_string();
    formatted_date
}

pub async fn scrape(
    constellation: String,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // 最初に最新の週でスクレイピングを試みる
    let first_attempt = scrape_with_date(&constellation, 0).await;

    // 404エラーだった場合のみ、前の週を試す
    match first_attempt {
        Ok(text) => Ok(text),
        Err(e) => {
            let error_string = e.to_string();
            if error_string.contains("HTTP error: 404") {
                // エラー文字列の検証後、新たな非同期呼び出しを行う
                scrape_with_date(&constellation, 1).await
            } else {
                // その他のエラーはそのまま返す
                Err(e)
            }
        }
    }
}

async fn scrape_with_date(
    constellation: &str,
    weeks_back: i64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let formatted_date = get_monday_date(weeks_back);
    let url = format!(
        "https://shiitakeuranai.jp/weekly-horoscope/{}/{}",
        formatted_date, constellation
    );

    let client = Client::new();
    let response = client.get(&url).send().await?;

    // エラーチェック
    if response.status() == 404 {
        return Err(format!(
            "HTTP error: 404 - {}の週は見つかりませんでした",
            formatted_date
        )
        .into());
    }

    if !response.status().is_success() {
        return Err(format!("HTTPエラー: {}", response.status()).into());
    }

    // 成功した場合はHTMLを解析
    let html_content = response.text().await?;
    let document = Html::parse_document(&html_content);
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
