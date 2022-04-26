use git2::{Cred, IndexAddOption, PushOptions, RemoteCallbacks, Repository};
use image::{
    codecs::pnm::{PnmSubtype, SampleEncoding},
    load_from_memory, GenericImage, GenericImageView, Rgb, RgbImage,
};
use serde::{Deserialize, Serialize};
use teloxide::{
    dispatching::{
        dialogue::{self, GetChatId, InMemStorage},
        UpdateFilterExt,
    },
    net::Download,
    payloads::SendMessageSetters,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode},
    utils::command::BotCommands,
};
use time::OffsetDateTime;
use tokio::{io::AsyncWriteExt, process::Command as TokioCommand};

use std::{env, error::Error, fs, io::Cursor, path::PathBuf, process::Stdio};

// const DCOS_SUPPORT_ID: i64 = 1638468462;
// const DCOS_RELEASES_ID: i64 = 1791772972;

const OTA_DCOS: &str = "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davinci.json";
const OTA_DCOS_PRE: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davinci_pre.json";
const OTA_DCOSX: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davincix.json";
const OTA_DCOSX_PRE: &str =
    "https://raw.githubusercontent.com/DavinciCodeOS/ota-data/main/davincix_pre.json";

const OVERLAY_GITLAB_PROJECT_ID: u64 = 35606329;

type LeonardoBot = AutoSend<Bot>;
type AppIconDialogue = Dialogue<State, InMemStorage<State>>;

#[derive(Clone)]
pub enum State {
    Start,
    ReceiveAppPath,
    ConfirmingAppPath {
        app_path: String,
    },
    ReceiveIconFile {
        app_path: String,
    },
    ReceiveIconName {
        app_path: String,
        file_id: String,
    },
    ReceiveDescription {
        app_path: String,
        file_id: String,
        icon_name: String,
    },
    ConfirmingCreation {
        vd_bytes: Vec<u8>,
        app_path: String,
        icon_name: String,
        description: String,
    },
}

impl Default for State {
    fn default() -> Self {
        Self::Start
    }
}

#[derive(BotCommands, Clone)]
#[command(rename = "lowercase", description = "These commands are supported:")]
enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "get the latest DCOS/DCOSX releases.")]
    Latest,
    #[command(description = "submit an icon for the pixel launcher overlay.")]
    AddIcon,
}

#[derive(Deserialize, Debug)]
struct OtaData {
    datetime: i64,
    url: String,
}

#[derive(Debug)]
struct AllReleases {
    dcos: Option<OtaData>,
    dcos_pre: Option<OtaData>,
    dcosx: Option<OtaData>,
    dcosx_pre: Option<OtaData>,
}

#[derive(Deserialize, Serialize, Debug)]
struct Icon {
    drawable: String,
    package: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct Icons {
    #[serde(rename = "icon")]
    icons: Vec<Icon>,
}

#[derive(Serialize, Debug)]
struct MergeRequestParams {
    id: u64,
    source_branch: String,
    target_branch: String,
    remove_source_branch: bool,
    title: String,
    description: String,
}

#[tokio::main]
async fn main() {
    let _ = dotenv::dotenv();
    pretty_env_logger::init();

    log::info!("Starting Leonardo");

    let client = reqwest::Client::new();
    let bot = Bot::from_env_with_client(client.clone()).auto_send();

    Dispatcher::builder(
        bot,
        dialogue::enter::<Update, InMemStorage<State>, State, _>()
            .branch(
                Update::filter_message()
                    .branch(teloxide::handler![State::ReceiveAppPath].endpoint(receive_app_path))
                    .branch(
                        teloxide::handler![State::ReceiveIconFile { app_path }]
                            .endpoint(receive_icon_file),
                    )
                    .branch(
                        teloxide::handler![State::ReceiveIconName { app_path, file_id }]
                            .endpoint(receive_icon_name),
                    )
                    .branch(
                        teloxide::handler![State::ReceiveDescription {
                            app_path,
                            file_id,
                            icon_name
                        }]
                        .endpoint(receive_description),
                    )
                    .branch(dptree::entry().filter_command::<Command>().endpoint(answer)),
            )
            .branch(
                Update::filter_callback_query()
                    .branch(
                        teloxide::handler![State::ConfirmingAppPath { app_path }]
                            .endpoint(receive_app_path_confirmation),
                    )
                    .branch(
                        teloxide::handler![State::ConfirmingCreation {
                            vd_bytes,
                            icon_name,
                            app_path,
                            description
                        }]
                        .endpoint(receive_creation_confirmation),
                    ),
            ),
    )
    .dependencies(dptree::deps![InMemStorage::<State>::new()])
    .build()
    .dispatch()
    .await;
}

async fn answer(
    bot: LeonardoBot,
    message: Message,
    command: Command,
    dialogue: AppIconDialogue,
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
        Command::AddIcon => {
            bot.send_message(message.chat.id, "Let's start! What is the app path of the app you want to add an icon for? For example com.discord or com.google.files").await?;

            dialogue.update(State::ReceiveAppPath).await?;
        }
    };

    Ok(())
}

async fn receive_app_path(
    bot: LeonardoBot,
    msg: Message,
    dialogue: AppIconDialogue,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(app_path) = msg.text() {
        if !app_path.contains('.') {
            bot.send_message(msg.chat.id, "App path should contain at least a '.', for example: com.discord or com.google.files").await?;

            return Ok(());
        }

        if !playstore_app_exists(bot.inner().client(), &app_path).await? {
            let answers = InlineKeyboardMarkup::default().append_row(
                vec!["Yes, this is correct", "No, this is wrong"]
                    .into_iter()
                    .map(|answer| {
                        InlineKeyboardButton::callback(answer.to_owned(), answer.to_owned())
                    }),
            );

            bot.send_message(msg.chat.id, "Could not find a playstore application with this name. Are you sure it is correct?").reply_markup(answers).await?;

            dialogue
                .update(State::ConfirmingAppPath {
                    app_path: app_path.to_owned(),
                })
                .await?;
        } else {
            bot.send_message(
                msg.chat.id,
                "Please attach a PNG with transparent background as the icon now.",
            )
            .await?;

            dialogue
                .update(State::ReceiveIconFile {
                    app_path: app_path.to_owned(),
                })
                .await?;
        }
    } else {
        bot.send_message(msg.chat.id, "Please send an app path.")
            .await?;
    }

    Ok(())
}

async fn receive_app_path_confirmation(
    bot: LeonardoBot,
    q: CallbackQuery,
    dialogue: AppIconDialogue,
    app_path: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(answer) = &q.data {
        if let Some(chat_id) = q.chat_id() {
            if answer == "Yes, this is correct" {
                bot.send_message(
                    chat_id,
                    "Please attach a PNG with transparent background as the icon now.",
                )
                .await?;

                dialogue.update(State::ReceiveIconFile { app_path }).await?;
            } else {
                bot.send_message(chat_id, "Aborting.").await?;

                dialogue.exit().await?;
            }
        }
    }

    Ok(())
}

async fn receive_icon_file(
    bot: LeonardoBot,
    msg: Message,
    dialogue: AppIconDialogue,
    app_path: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(document) = msg.document() {
        bot.send_message(
            msg.chat.id,
            "Provide a name for this icon, for example youtube_music or whatsapp.",
        )
        .await?;

        dialogue
            .update(State::ReceiveIconName {
                app_path,
                file_id: document.file_id.clone(),
            })
            .await?;
    } else {
        bot.send_message(msg.chat.id, "Please attach an image.")
            .await?;
    }

    Ok(())
}

async fn receive_icon_name(
    bot: LeonardoBot,
    msg: Message,
    dialogue: AppIconDialogue,
    (app_path, file_id): (String, String),
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(name) = msg.text() {
        bot.send_message(
            msg.chat.id,
            "Finally, provide a short description for this request.",
        )
        .await?;

        dialogue
            .update(State::ReceiveDescription {
                app_path,
                file_id,
                icon_name: name.to_owned(),
            })
            .await?;
    } else {
        bot.send_message(msg.chat.id, "Please provide a name.")
            .await?;
    }

    Ok(())
}

async fn receive_description(
    bot: LeonardoBot,
    msg: Message,
    dialogue: AppIconDialogue,
    (app_path, file_id, icon_name): (String, String, String),
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let description = msg.text().unwrap_or_default();

    let bot_msg = bot
        .send_message(msg.chat.id, "Downloading image...")
        .await?;

    let file = bot.get_file(file_id).await?;

    let mut file_bytes = Vec::new();
    bot.download_file(&file.file_path, &mut file_bytes).await?;

    bot.edit_message_text(msg.chat.id, bot_msg.id, "Converting PNG to black PNM...")
        .await?;

    let pnm_pixel_bytes = tokio::task::spawn_blocking(move || {
        let mut img = load_from_memory(&file_bytes)?;
        let mut out_img = RgbImage::new(img.width(), img.height());

        for y in 0..img.height() {
            for x in 0..img.width() {
                // Convert any pixels that are not transparent to black
                let pixel = img.get_pixel(x, y);

                if pixel.0[3] > 0 {
                    out_img.put_pixel(x, y, Rgb::<u8>([0, 0, 0]));
                } else {
                    out_img.put_pixel(x, y, Rgb::<u8>([255, 255, 255]));
                }

                img.put_pixel(x, y, pixel);
            }
        }

        let mut out = Vec::new();

        out_img.write_to(
            &mut Cursor::new(&mut out),
            image::ImageOutputFormat::Pnm(PnmSubtype::Pixmap(SampleEncoding::Binary)),
        )?;

        Ok::<_, Box<dyn Error + Send + Sync>>(out)
    })
    .await??;

    bot.edit_message_text(msg.chat.id, bot_msg.id, "Tracing PNM to SVG...")
        .await?;

    let mut potrace_proc = TokioCommand::new("potrace");
    potrace_proc.arg("--svg");
    potrace_proc.stdout(Stdio::piped());
    potrace_proc.stdin(Stdio::piped());

    let mut child = potrace_proc.spawn()?;
    let mut stdin = child.stdin.take().unwrap();

    stdin.write_all(&pnm_pixel_bytes).await?;
    drop(stdin);

    let op = child.wait_with_output().await?;

    if !op.status.success() {
        bot.edit_message_text(msg.chat.id, bot_msg.id, "Failed to trace PNM to SVG.")
            .await?;

        return Ok(());
    } else {
        bot.edit_message_text(msg.chat.id, bot_msg.id, "Converting SVG to VD...")
            .await?;
    }

    let svg_bytes = op.stdout;

    let mut vd_proc = TokioCommand::new("svg2vd");
    vd_proc.args(&["-i", "-", "-o", "-"]);
    vd_proc.stdout(Stdio::piped());
    vd_proc.stdin(Stdio::piped());

    let mut child = vd_proc.spawn()?;
    let mut stdin = child.stdin.take().unwrap();

    stdin.write_all(&svg_bytes).await?;
    drop(stdin);

    let op = child.wait_with_output().await?;

    if !op.status.success() {
        bot.edit_message_text(msg.chat.id, bot_msg.id, "Failed to convert SVG to VD.")
            .await?;

        return Ok(());
    } else {
        bot.edit_message_text(
            msg.chat.id,
            bot_msg.id,
            "Done with conversion. Here's a preview of the SVG:",
        )
        .await?;
    }

    let vd_bytes = op.stdout;

    let answers = InlineKeyboardMarkup::default().append_row(
        vec!["Yes, create my request", "No, abort"]
            .into_iter()
            .map(|answer| InlineKeyboardButton::callback(answer.to_owned(), answer.to_owned())),
    );

    bot.send_document(
        msg.chat.id,
        InputFile::memory(svg_bytes).file_name("icon.svg"),
    )
    .caption("Please review the SVG file and if it is good, proceed!")
    .reply_markup(answers)
    .await?;

    dialogue
        .update(State::ConfirmingCreation {
            vd_bytes,
            app_path,
            description: description.to_owned(),
            icon_name,
        })
        .await?;

    Ok(())
}

async fn receive_creation_confirmation(
    bot: LeonardoBot,
    q: CallbackQuery,
    dialogue: AppIconDialogue,
    (vd_bytes, icon_name, app_path, description): (Vec<u8>, String, String, String),
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(answer) = &q.data {
        if let Some(chat_id) = q.chat_id() {
            if answer == "Yes, create my request" {
                let base = env::var("PATH_TO_ICONS_OVERLAY")?;
                let branch_name = format!("bot/icon_{icon_name}");
                let branch_refspec = format!("refs/heads/{branch_name}");
                let vd_file_name = format!("themed_icon_{icon_name}.xml");
                let commit_msg = format!("overlay: Add icon for {icon_name}");

                let commit_msg_clone = commit_msg.clone();
                let branch_name_clone = branch_name.clone();

                let vd_file_path: PathBuf = [
                    &base,
                    "PixelLauncherIconsOverlay",
                    "res",
                    "drawable",
                    &vd_file_name,
                ]
                .iter()
                .collect();
                let xml_file_path: PathBuf = [
                    &base,
                    "PixelLauncherIconsOverlay",
                    "res",
                    "xml",
                    "grayscale_icon_map.xml",
                ]
                .iter()
                .collect();

                tokio::task::spawn_blocking(move || {
                    let prev_xml = fs::read_to_string(&xml_file_path)?;

                    // For whatever reason, none of the XML parsers for Rust have proper
                    // support for serde + pretty serialization.
                    // So for now, we add the line where it is needed.
                    let mut lines: Vec<String> = prev_xml.lines().map(ToString::to_string).collect();
                    let line = format!("    <icon drawable=\"@drawable/themed_icon_{icon_name}\" package=\"{app_path}\" />");
                    lines.insert(2, line);
                    let line_count = lines.len();
                    lines[2..line_count - 1].sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

                    let new_xml = lines.join("\n");

                    fs::write(vd_file_path, vd_bytes)?;
                    fs::write(xml_file_path, new_xml)?;

                    let repo = Repository::open(base)?;

                    let head = repo.head()?.peel_to_commit()?;
                    let branch = repo.branch(&branch_name, &head, true)?;
                    repo.set_head(branch.into_reference().name().unwrap())?;

                    let tree_id = {
                        let mut index = repo.index()?;
                        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
                        index.write_tree()?
                    };
                    let tree = repo.find_tree(tree_id)?;

                    let signature = repo.signature()?;
                    let head = repo.head()?.peel_to_commit()?;
                    repo.commit(Some("HEAD"), &signature, &signature, &commit_msg, &tree, &[&head])?;
                    repo.checkout_head(None)?;

                    let mut push_opts = PushOptions::new();
                    let mut callbacks = RemoteCallbacks::new();
                    let mut remote = repo.find_remote("origin")?;
                    callbacks.credentials(|_url, username_from_url, _allowed_type| {
                        Cred::ssh_key_from_agent(username_from_url.unwrap_or("git"))
                    });
                    push_opts.remote_callbacks(callbacks);
                    remote.push(&[&branch_refspec], Some(&mut push_opts))?;

                    let main_ref = repo.revparse_single("12.1")?;
                    repo.checkout_tree(&main_ref, None)?;
                    repo.set_head("refs/heads/12.1")?;

                    Ok::<(), Box<dyn Error + Send + Sync>>(())
                })
                .await??;

                let params = MergeRequestParams {
                    id: OVERLAY_GITLAB_PROJECT_ID,
                    title: commit_msg_clone,
                    description,
                    source_branch: branch_name_clone,
                    target_branch: String::from("12.1"),
                    remove_source_branch: true,
                };

                bot.inner().client()
                    .post(format!("https://gitlab.com/api/v4/projects/{OVERLAY_GITLAB_PROJECT_ID}/merge_requests"))
                    .header("PRIVATE-TOKEN", env::var("GITLAB_TOKEN")?)
                    .json(&params)
                    .send().await?;

                bot.send_message(chat_id, "Created.").await?;
            } else {
                bot.send_message(chat_id, "Aborting.").await?;
            }

            dialogue.exit().await?;
        }
    }

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

async fn playstore_app_exists(
    client: &reqwest::Client,
    app_path: &str,
) -> Result<bool, reqwest::Error> {
    let app_url = format!("https://play.google.com/store/apps/details?id={app_path}&gl=US");

    Ok(client.head(app_url).send().await?.status() == 200)
}
