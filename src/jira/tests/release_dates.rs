use super::{jira_settings, *};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn load_with_project_versions(
    status: &str,
    versions: Value,
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
            let mut buffer = [0; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap();
            let mut response_status = "200 OK";
            let body = if path == "/rest/agile/1.0/board/42" {
                json!({"id":42,"name":"Finery","type":"scrum"})
            } else if path.contains("velocity.json") {
                json!({"sprints":[],"velocityStatEntries":{}})
            } else if path.contains("/board/42/sprint") {
                json!({"values":[{"id":7,"name":"Sprint 7","state":"active",
                    "startDate":"2026-08-31T09:00:00Z","endDate":"2026-09-14T09:00:00Z"}],
                    "isLast":true,"startAt":0,"maxResults":50})
            } else if path.contains("/sprint/7/issue") {
                json!({"issues":[issue("FIN-1", "1")],"isLast":true})
            } else if path.contains("/board/42/backlog") {
                json!({"issues":[issue("FIN-2", "1"),issue("FIN-3", "2")],"isLast":true})
            } else if path == "/rest/api/3/project/FIN/versions" {
                response_status = &status;
                versions.clone()
            } else {
                panic!("Unexpected request: {path}");
            }
            .to_string();
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

fn issue(key: &str, version_id: &str) -> Value {
    json!({"key":key,"fields":{"summary":key,"issuetype":{"name":"Story"},
        "customfield_10016":3,"fixVersions":[{"id":version_id,"name":"v1.0",
            "releaseDate":"2026-10-29"}]}})
}

#[test]
fn backlog_hydrates_release_dates_by_id_once_per_project() {
    let (loaded, requests) = load_with_project_versions(
        "200 OK",
        json!([
            {"id":"2","name":"v1.0","startDate":"2026-09-20","releaseDate":"2026-10-29"},
            {"id":"1","name":"v1.0","startDate":"2026-09-12","releaseDate":"2026-09-19"}
        ]),
    );
    let items = loaded
        .snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&loaded.snapshot.work_items)
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 3);
    for item in items {
        let version = &item.releases[0];
        let (start, end) = if version.id == "1" {
            ("2026-09-12", "2026-09-19")
        } else {
            ("2026-09-20", "2026-10-29")
        };
        assert_eq!(
            version.start_date,
            crate::store::work_items::release::parse_date(start),
            "{}",
            item.key
        );
        assert_eq!(
            version.end_date,
            crate::store::work_items::release::parse_date(end),
            "{}",
            item.key
        );
    }
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("/project/FIN/versions"))
            .count(),
        1
    );
}

#[test]
fn release_date_lookup_failure_keeps_backlog_and_reports_warning() {
    let (loaded, _) = load_with_project_versions(
        "503 Service Unavailable",
        json!({"errorMessages":["Versions unavailable"]}),
    );
    assert_eq!(loaded.snapshot.work_items.len(), 2);
    assert!(
        loaded
            .snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("release dates") && warning.contains("FIN"))
    );
}
