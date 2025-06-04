use std::collections::VecDeque;

#[cfg(feature = "server")]
use dioxus::logger::tracing::warn;
use dioxus::prelude::{
    server_fn::codec::{StreamingText, TextStream},
    *,
};

use futures::StreamExt as _;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
//const HEADER_SVG: Asset = asset!("/assets/header.svg");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[cfg(feature = "server")]
mod nats_utilities {
    use tokio::sync::OnceCell;

    static JETSTREAM: OnceCell<async_nats::jetstream::Context> = OnceCell::const_new();

    async fn jetstream() -> async_nats::jetstream::Context {
        let nats_client = async_nats::connect(
            std::env::var("NATS_URL").unwrap_or("nats://localhost:4222".into()),
        )
        .await
        .unwrap();
        async_nats::jetstream::new(nats_client)
    }

    pub async fn get_jetstream() -> &'static async_nats::jetstream::Context {
        JETSTREAM.get_or_init(jetstream).await
    }
}

// Discussion about using toikio for async server functions:
// https://github.com/DioxusLabs/dioxus/discussions/2310

// State management:
// https://bcnrust.github.io/devbcn-workshop/frontend/03_04_state_management.html

fn main() {
    // Init logger at INFO level to suppress NATS DEBUG output
    dioxus_logger::init(dioxus_logger::tracing::Level::INFO).expect("failed to init logger");
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        MessageViewer {}
    }
}

fn process_message(msg: String) -> String {
    if msg.starts_with("matrix") {
        let res = msg
            .chars()
            .skip_while(|c| !c.is_digit(10))
            .collect::<String>();
        if !res.is_empty() {
            let n = res.parse::<i32>().unwrap();
            let mut a = ndarray::Array::eye(3);
            a *= n;
            return format!("Matrix: \n{}", a);
        }
    }
    msg
}

#[component]
pub fn MessageViewer() -> Element {
    let mut msgs: Signal<VecDeque<String>> = use_signal(|| VecDeque::new());
    let mut subject = use_signal(|| String::from(""));
    let mut task_handle: Option<Task> = None;

    let submit_handler = move |evt: Event<FormData>| {
        // A new subject is submitted: update the state and start a new async task to receive messages
        let new_subject = evt.values()["subject"].as_value();
        subject.set(new_subject.to_string());
        msgs.set(VecDeque::new()); // Clear previous messages

        // Cancel the previous async task if it exists
        if let Some(existing_task) = task_handle {
            existing_task.cancel();
            dioxus_logger::tracing::info!("Cancelled previous task for subject: {}", new_subject);
        }
        // No point in subscribing to an empty subject
        if new_subject.is_empty() {
            return;
        }
        // Spawn a new async task to receive messages on the new subject
        task_handle = Some(spawn(async move {
            if let Ok(stream) = msg_stream(new_subject).await {
                let mut stream = stream.into_inner();
                while let Some(Ok(s)) = stream.next().await {
                    let mut msgs = msgs.write();
                    if msgs.len() >= 3 {
                        msgs.pop_back(); // Keep only the last 10 messages
                    }
                    msgs.push_front(process_message(s));
                }
            }
        }));
    };

    rsx! {
        div { class: "container",
            header {
                h1 { "Live Message Display" }
            }
            main {
                section { class: "form-section",
                    h2 { "Message Selector" }
                    form { id: "messageSelectorForm", onsubmit: submit_handler,
                        div { class: "form-group",
                            label { r#for: "subject", "Subject:" }
                            input {
                                id: "subjectInput",
                                name: "subject",
                                placeholder: "e.g., alerts.critical, sensor.temp.*",
                                r#type: "text",
                            }
                        }
                        button { r#type: "submit", "Watch Subject" }
                    }
                }
                ReceivedMessages { subject, msgs: msgs() }
            }
            footer {
                p { "© Various Robots 2025" }
            }
        }
    }
}

#[component]
pub fn ReceivedMessages(subject: String, msgs: VecDeque<String>) -> Element {
    rsx! {
        section { class: "messages-section",
            h3 {
                "Received Messages (Subject: "
                span { id: "currentSubjectDisplay", {subject.clone()} }
                ")"
            }
            div { class: "highlighted-area", id: "messageDisplayArea",
                if subject.is_empty() {
                    p { class: "placeholder-message",
                        "No messages received yet. Please enter a subject and click \"Watch Subject\"."
                    }
                } else {
                    for msg in msgs {
                        p { class: "message-item", {msg} }
                    }
                }
            }
        }
    }
}

#[server(output = StreamingText)]
async fn msg_stream(subject: String) -> Result<TextStream, ServerFnError> {
    let jetstream = nats_utilities::get_jetstream().await;
    let kv = jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "messages".to_string(),
            ..Default::default()
        })
        .await?;

    println!("watching subject: {}", &subject);

    let (tx, rx) = futures::channel::mpsc::unbounded();
    let mut watch = kv.watch(subject).await?;

    tokio::spawn(async move {
        while let Some(msg) = watch.next().await {
            match msg {
                Ok(entry) => {
                    if tx
                        .unbounded_send(Ok(std::str::from_utf8(&entry.value).unwrap().to_string()))
                        .is_err()
                    {
                        warn!(" msg_stream() : stream close");
                        tx.close_channel();
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("Error receiving message: {}", e);
                }
            }
        }
    });

    Ok(TextStream::new(rx))
}
