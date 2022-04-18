use std::error::Error;

use serde::Deserialize;
use teloxide::{
    dispatching::{update_listeners, UpdateFilterExt},
    payloads::SendMessageSetters,
    prelude::*,
    types::ParseMode,
    utils::command::BotCommands,
};
use time::OffsetDateTime;

const DCOS_SUPPORT_ID: i64 = 1638468462;
const DCOS_RELEASES_ID: i64 = 1791772972;

const OTA_DCOS: &str = "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davinci.json";
const OTA_DCOS_PRE: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davinci_pre.json";
const OTA_DCOSX: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davincix.json";
const OTA_DCOSX_PRE: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davincix_pre.json";

type LeonardoBot = AutoSend<Bot>;

#[derive(BotCommands, Clone)]
#[command(rename = "lowercase", description = "These commands are supported:")]
enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "get the latest DCOS/DCOSX releases.")]
    Latest,
}

#[derive(Deserialize, Debug)]
struct OtaData {
    error: bool,
    version: String,
    maintainers: Vec<Maintainer>,
    donate_url: String,
    website_url: String,
    news_url: String,
    datetime: i64,
    filename: String,
    id: String,
    size: u64,
    url: String,
    filehash: String,
}

#[derive(Deserialize, Debug)]
struct Maintainer {
    main_maintainer: bool,
    github_username: String,
    name: String,
}

#[derive(Debug)]
struct AllReleases {
    dcos: Option<OtaData>,
    dcos_pre: Option<OtaData>,
    dcosx: Option<OtaData>,
    dcosx_pre: Option<OtaData>,
}

#[tokio::main]
async fn main() {
    let _ = dotenv::dotenv();
    pretty_env_logger::init();

    log::info!("Starting Leonardo");

    let client = reqwest::Client::new();
    let bot = Bot::from_env_with_client(client.clone()).auto_send();

    Dispatcher::builder(
        bot.clone(),
        Update::filter_message()
            .filter_command::<Command>()
            .chain(dptree::endpoint(answer)),
    )
    .build()
    .dispatch_with_listener(
        update_listeners::polling_default(bot).await,
        LoggingErrorHandler::with_custom_text("An error from the update listener"),
    )
    .await;
}

async fn answer(
    bot: LeonardoBot,
    message: Message,
    command: Command,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Latest => {
            let releases = get_latest_releases(bot.inner().client()).await?;

            let mut text = String::new();

            for (name, data) in [
                ("DCOS \\(stable\\)", releases.dcos),
                ("DCOS \\(pre\\-release\\)", releases.dcos_pre),
                ("DCOSX \\(stable\\)", releases.dcosx),
                ("DCOSX \\(pre\\-release\\)", releases.dcosx_pre),
            ]
            .into_iter()
            {
                if let Some(release) = data {
                    let dt = OffsetDateTime::from_unix_timestamp(release.datetime)?;
                    let format = time::format_description::parse(
                        "[year]\\-[month]\\-[day] [hour]:[minute]:[second]",
                    )?;
                    let timestamp = dt.format(&format)?;
                    let desc = format!(
                        "{}: [download]({}) \\(Updated {}\\)\n",
                        name, release.url, timestamp
                    );
                    text.push_str(&desc);
                } else {
                    text.push_str(name);
                    text.push_str(": no release available\n");
                }
            }

            bot.send_message(message.chat.id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
        }
    };

    Ok(())
}

async fn get_release(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<OtaData>, reqwest::Error> {
    Ok(client.get(url).send().await?.json::<OtaData>().await.ok())
}

async fn get_latest_releases(client: &reqwest::Client) -> Result<AllReleases, reqwest::Error> {
    let dcos = get_release(client, OTA_DCOS).await?;
    let dcos_pre = get_release(client, OTA_DCOS_PRE).await?;
    let dcosx = get_release(client, OTA_DCOSX).await?;
    let dcosx_pre = get_release(client, OTA_DCOSX_PRE).await?;

    Ok(AllReleases {
        dcos,
        dcos_pre,
        dcosx,
        dcosx_pre,
    })
}
