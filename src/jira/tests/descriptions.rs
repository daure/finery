use super::{jira_settings, read_request, *};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const WIKI_DESCRIPTION: &str = "h1. Details\n\n* A *bold* point\n\n{noformat}fn main() {}{noformat}\n\n{quote}Important{quote}";

fn rich_description() -> Value {
    json!({"type":"doc","version":1,"content":[
        {"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"Details"}]},
        {"type":"bulletList","content":[{"type":"listItem","content":[
            {"type":"paragraph","content":[{"type":"text","text":"A "},
                {"type":"text","text":"bold","marks":[{"type":"strong"}]},
                {"type":"text","text":" point"}]}
        ]}]},
        {"type":"codeBlock","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn main() {}"}]},
        {"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"Important"}]}]}
    ]})
}

fn load_descriptions(
    status: &str,
    descriptions: Value,
) -> (super::super::BacklogLoad, Vec<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut settings = jira_settings(format!("http://{}", listener.local_addr().unwrap()));
    settings.jira_default_board = "42".into();
    settings.backlog_runway.use_jira_velocity = false;
    listener.set_nonblocking(true).unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let stop = finished.clone();
    let status = status.to_owned();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&mut stream);
            let path = request.split_whitespace().nth(1).unwrap();
            let mut response_status = "200 OK";
            let body = if path == "/rest/agile/1.0/board/42" {
                json!({"id":42,"name":"Finery","type":"scrum"})
            } else if path.contains("velocity.json") {
                json!({"sprints":[],"velocityStatEntries":{}})
            } else if path.contains("/board/42/sprint") {
                json!({"values":[{"id":7,"name":"Sprint 7","state":"active"}],"isLast":true,"startAt":0,"maxResults":50})
            } else if path.contains("/sprint/7/issue") {
                json!({"issues":[agile_issue("FIN-1", WIKI_DESCRIPTION)],"isLast":true})
            } else if path.contains("/board/42/backlog") {
                json!({"issues":[agile_issue("FIN-2", WIKI_DESCRIPTION), agile_issue("FIN-3", "")],"isLast":true})
            } else if path == "/rest/api/3/issue/bulkfetch" {
                response_status = &status;
                descriptions.clone()
            } else {
                panic!("Unexpected request: {path}");
            }.to_string();
            requests.push(request);
            write!(stream, "HTTP/1.1 {response_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let result = super::super::backlog(&settings);
    finished.store(true, Ordering::Relaxed);
    let requests = server.join().unwrap();
    (result.unwrap(), requests)
}

fn agile_issue(key: &str, description: &str) -> Value {
    json!({"key":key,"fields":{"summary":key,"issuetype":{"name":"Story"},
        "description":description,"customfield_10016":3}})
}

#[test]
fn backlog_and_sprint_descriptions_use_composer_markdown() {
    let adf = rich_description();
    let (loaded, requests) = load_descriptions(
        "200 OK",
        json!({"issues":[
            {"key":"FIN-2","fields":{"description":adf}},
            {"key":"FIN-1","fields":{"description":adf}}
        ]}),
    );
    let composer = to_ticket(JiraIssue {
        key: "FIN-1".into(),
        fields: json!({"description":adf}),
    });
    assert_eq!(
        composer.description,
        "# Details\n\n- A **bold** point\n\n```rust\nfn main() {}\n```\n\n> Important"
    );
    assert_eq!(
        loaded.snapshot.sprints[0].work_items[0].description,
        composer.description
    );
    assert_eq!(
        loaded.snapshot.work_items[0].description,
        composer.description
    );
    assert!(loaded.snapshot.work_items[1].description.is_empty());
    assert!(loaded.snapshot.warnings.is_empty());
    let lookups = requests
        .iter()
        .filter(|request| request.starts_with("POST /rest/api/3/issue/bulkfetch"))
        .collect::<Vec<_>>();
    assert_eq!(lookups.len(), 1);
    let payload: Value =
        serde_json::from_str(lookups[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        payload,
        json!({"issueIdsOrKeys":["FIN-1","FIN-2"],"fields":["description"]})
    );
}

#[test]
fn description_lookup_failure_keeps_backlog_readable_with_a_warning() {
    for (status, response) in [
        (
            "503 Service Unavailable",
            json!({"errorMessages":["Unavailable"]}),
        ),
        ("200 OK", json!({"issues":[]})),
    ] {
        let (loaded, _) = load_descriptions(status, response);
        assert_eq!(
            loaded.snapshot.sprints[0].work_items[0].description,
            WIKI_DESCRIPTION
        );
        assert_eq!(loaded.snapshot.work_items[0].description, WIKI_DESCRIPTION);
        assert!(
            loaded
                .snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("descriptions"))
        );
    }
}
