use wit_bindgen::generate;
use wstd::http::{Body, Error, HeaderValue, Method, Request, Response, StatusCode};

generate!({
    world: "imports",
    path: "../../wit",
});

const TOKEN: &str = "rvmtour-demo-token";
const QUIC_ENDPOINT: &str = "127.0.0.1:8086";

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, Error> {
    let path = req.uri().path();

    match (req.method(), path) {
        (&Method::GET, "/") => home(),
        (&Method::GET, "/health") => {
            json(StatusCode::OK, r#"{"ok":true,"service":"game-service"}"#)
        }
        (&Method::POST, "/login") => login(req),
        (&Method::GET, "/me") => me(req),
        (&Method::GET, "/races") => races(req),
        (&Method::GET, "/history/workouts") => history_list(req, "workouts", "workouts"),
        (&Method::GET, "/history/races") => history_list(req, "races", "races"),
        (&Method::GET, "/history/stats") => history_list(req, "stats", "stats"),
        (&Method::GET, "/matchmaking/queue") => matchmaking_queue(req),
        (&Method::POST, "/matchmaking/join") => matchmaking_join(req),
        (&Method::GET, "/session/bootstrap") => session_bootstrap(req),
        (&Method::GET, path) if path.starts_with("/history/workouts/") => {
            history_detail(req, "workouts", "/history/workouts/")
        }
        (&Method::GET, path) if path.starts_with("/history/races/") => {
            history_detail(req, "races", "/history/races/")
        }
        _ => not_found(),
    }
}

fn home() -> Result<Response<Body>, Error> {
    json(
        StatusCode::OK,
        r#"{"service":"RVM Tour control plane","routes":["POST /login","GET /me","GET /races","GET /history/workouts","GET /history/races","GET /history/stats","GET /history/workouts/{id}","GET /history/races/{id}","GET /matchmaking/queue","POST /matchmaking/join","GET /session/bootstrap"]}"#,
    )
}

fn login(req: Request<Body>) -> Result<Response<Body>, Error> {
    let rider = query_param(req.uri().query(), "rider")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "guest-rider".to_owned());
    let rider_id = stable_rider_id(&rider);

    json(
        StatusCode::OK,
        &format!(
            r#"{{
  "token":"{TOKEN}",
  "rider":{{"id":{rider_id},"name":"{}","ftp":255,"weightKg":73}},
  "entitlements":["race","free-ride","matchmaking"],
  "next":"GET /session/bootstrap"
}}"#,
            json_escape(&rider)
        ),
    )
}

fn me(req: Request<Body>) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    json(
        StatusCode::OK,
        r#"{"rider":{"id":4242,"name":"Demo Rider","ftp":255,"weightKg":73},"stats":{"level":18,"weeklyDistanceKm":86.4,"raceRating":612}}"#,
    )
}

fn races(req: Request<Body>) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    json(
        StatusCode::OK,
        r#"{"races":[{"id":"crit-city-1830","name":"RVM Crit City","route":"Neon Loop","startsInSeconds":420,"distanceMeters":1200,"laps":8,"category":"C","registered":18},{"id":"tempo-tuesday-1900","name":"Tempo Tuesday","route":"Harbor Rollers","startsInSeconds":2220,"distanceMeters":9600,"laps":4,"category":"B","registered":42},{"id":"climb-lab-2000","name":"Climb Lab","route":"Switchback Test","startsInSeconds":5820,"distanceMeters":7400,"laps":1,"category":"Open","registered":11}]}"#,
    )
}

fn history_list(
    req: Request<Body>,
    namespace: &str,
    response_field: &str,
) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    match rvm::lambda::host::kv_list(namespace, "") {
        Ok(items) => {
            let values = items
                .into_iter()
                .map(|(_key, value)| value)
                .collect::<Vec<_>>()
                .join(",");
            json(
                StatusCode::OK,
                &format!(r#"{{"{response_field}":[{values}]}}"#),
            )
        }
        Err(err) => json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!(
                r#"{{"error":"kv list failed","message":"{}"}}"#,
                json_escape(&err)
            ),
        ),
    }
}

fn history_detail(
    req: Request<Body>,
    namespace: &str,
    path_prefix: &str,
) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    let Some(id) = req.uri().path().strip_prefix(path_prefix) else {
        return not_found();
    };
    if id.is_empty() {
        return not_found();
    }

    let id = percent_decode(id);
    match rvm::lambda::host::kv_get(namespace, &id) {
        Ok(Some(value)) => json(StatusCode::OK, &value),
        Ok(None) => json(
            StatusCode::NOT_FOUND,
            &format!(
                r#"{{"error":"history item not found","id":"{}"}}"#,
                json_escape(&id)
            ),
        ),
        Err(err) => json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!(
                r#"{{"error":"kv get failed","message":"{}"}}"#,
                json_escape(&err)
            ),
        ),
    }
}

fn matchmaking_queue(req: Request<Body>) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    json(
        StatusCode::OK,
        r#"{"queue":{"mode":"ranked-crit","status":"forming","ridersWaiting":7,"estimatedWaitSeconds":95,"targetFieldSize":12,"category":"C"}}"#,
    )
}

fn matchmaking_join(req: Request<Body>) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    let race_id =
        query_param(req.uri().query(), "race").unwrap_or_else(|| "crit-city-1830".to_owned());
    json(
        StatusCode::ACCEPTED,
        &format!(
            r#"{{"ticket":"mm-{}-4242","raceId":"{}","status":"queued","poll":"GET /matchmaking/queue"}}"#,
            json_escape(&race_id),
            json_escape(&race_id)
        ),
    )
}

fn session_bootstrap(req: Request<Body>) -> Result<Response<Body>, Error> {
    if !is_authorized(&req) {
        return unauthorized();
    }

    json(
        StatusCode::OK,
        &format!(
            r#"{{
  "quicEndpoint":"{QUIC_ENDPOINT}",
  "world":{{"routeLengthMeters":1200,"tickHz":20,"course":"Neon Loop"}},
  "matchmaking":{{"defaultRaceId":"crit-city-1830","join":"POST /matchmaking/join?race=crit-city-1830"}},
  "assets":{{"bike":"demo-road","kit":"rvm-orange"}}
}}"#
        ),
    )
}

fn unauthorized() -> Result<Response<Body>, Error> {
    json(
        StatusCode::UNAUTHORIZED,
        r#"{"error":"missing bearer token","hint":"POST /login?rider=your-name and send Authorization: Bearer rvmtour-demo-token"}"#,
    )
}

fn not_found() -> Result<Response<Body>, Error> {
    json(
        StatusCode::NOT_FOUND,
        r#"{"error":"not found","routes":["POST /login","GET /races","POST /matchmaking/join","GET /session/bootstrap"]}"#,
    )
}

fn json(status: StatusCode, body: &str) -> Result<Response<Body>, Error> {
    Ok(Response::builder()
        .status(status)
        .header("content-type", HeaderValue::from_static("application/json"))
        .body(body.to_owned().into())?)
}

fn is_authorized(req: &Request<Body>) -> bool {
    req.headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {TOKEN}"))
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let mut out = String::new();
    let mut bytes = value.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        match byte {
            b'+' => out.push(' '),
            b'%' => {
                let hi = bytes.next();
                let lo = bytes.next();
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    if let Some(decoded) = from_hex(hi).zip(from_hex(lo)).map(|(h, l)| h * 16 + l) {
                        out.push(decoded as char);
                    }
                }
            }
            _ => out.push(byte as char),
        }
    }
    out
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn stable_rider_id(name: &str) -> u32 {
    name.bytes().fold(1729_u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    })
}
