use wit_bindgen::generate;
use crate::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::http::types::{Fields, OutgoingResponse, ResponseOutparam},
};

generate!({
    world: "rvm",
    path: "../../wit",
    generate_all
});

struct MyGuest;

impl Guest for MyGuest {
    fn handle(request: wasi::http::types::IncomingRequest, response_out: ResponseOutparam) -> () {
        if let Some("/echo") = request.path_with_query().as_deref() {
            let resp = OutgoingResponse::new(Fields::new());
            
            if let Ok(out_body) = resp.body() {
                if let Ok(out_stream) = out_body.write() {
                    if let Ok(in_body) = request.consume() {
                        if let Ok(in_stream) = in_body.stream() {
                            while let Ok(chunk) = in_stream.read(100) {
                                let _ = out_stream.write(&chunk);
                            }
                            let _ = out_stream.flush();
                        }
                    }
                }
                let _ = wasi::http::types::OutgoingBody::finish(out_body, None);
            }
            let _ = resp.set_status_code(200);
            ResponseOutparam::set(response_out, Ok(resp));
            return;
        }

        if let Some("/secret") = request.path_with_query().as_deref() {
            let resp = OutgoingResponse::new(Fields::new());
            if let Ok(out_body) = resp.body() {
                if let Ok(out_stream) = out_body.write() {
                    let _ = out_stream.write(rvm::lambda::host::client_secret().as_bytes());
                }
                let _ = wasi::http::types::OutgoingBody::finish(out_body, None);
            }
            let _ = resp.set_status_code(200);
            ResponseOutparam::set(response_out, Ok(resp));
            return;
        }
        
        ResponseOutparam::set(response_out, Err(wasi::http::types::ErrorCode::DestinationNotFound));
    }
}

export! {
    MyGuest
}