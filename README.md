# RVM
RVM is a service where you can upload guest services and execute them in a sandbox.
Guests can use either HTTP or QUIC.

The main purpose is as a _toy_ cloud platform for multiplayer apps and games.

The guests have the following limits:
* Limit of `268 KiB` of memory per guests
* `100_000_000` of starting fuel for each guest.

## Quickstart

Start service: `cargo run --release` The service automatically starts any module in the `module-store` folder.

Invoke: `curl -X GET -i http://127.0.0.1:8000/python-svc/secret`

# Writing guests for RVM
You can write guests in any language you want, as long as it compiles to webassembly.
This example uses python.
There are other rust examples in the repo that are more straight forward, those can simply be compiled with something like `cargo build -p http-rust --target wasm32-wasip2`.

### 1. Implement

Write your app and import the things you need.
There's an example in `guests/http-python/http_server.py` that implements a HTTP server that can be run in RVM.
Right now, the RVM expects all guests to be a HTTP proxy.
Every time it receives an `invoke` request it will run `IncomingHandler::handle` in your guest, with a forwarded HTTP request.

### 2. Build
1. Make sure you have `componentize-py`, which can be installed via `pip install componentize-py`
2. Build the wasm module `pushd guests/http-python && componentize-py -d ../../wit -w rvm componentize http_server -o http-python.wasm` && popd

### 3. Deploy
`curl --data-binary "@guests/http-python/http-python.wasm" localhost:8002/deploy/http-python` 

### 4. Talk to your deployed app

* Get the SHA of some page: - `curl -X GET -i -H 'url: https://webassembly.github.io/spec/core/' http://127.0.0.1:8000/http-python/hash-all`
* Echo back a body - `curl -X POST -i http://127.0.0.1:8000/http-python/echo -d "xd"`
* Print a secret provided by the host - `curl -X GET -i http://127.0.0.1:8000/http-python/secret`

# Extending RVM

### Adding new host functions (i.e. functions that guests can call)
Edit the `host` import in `wit/world.wit`, which will be imported by the `rvm` world.
Edit the `rvm::lambda::host::Host` impl for `HostComponent` and add your newly added function.
The rust code will automatically run bindgen when compiled.

After you are done with the host changes, you need to make the changes available for the python guest by regenerating the bindings.
 1. Make sure you have `componentize-py`, which can be installed via:

     `pip install componentize-py`.
 2. Generate bindings to output dir `guests`:

    `componentize-py -d wit -w rvm bindings guests/http-python`
 3. Import your added function in your python code
    ```python
    from rvm.imports.host import (
        client_secret,
    )
    ```
4. Call your function! `secret = client_secret()`
   This will run the `client_secret` function from the host and return the response to your guest.

## Changing backing store
We use OpenDAL, so switching backing store to something that's not the file system only requires you to use another service.

## Cheat sheet (debug build)

```
cargo r
cargo b -p http-rust --target wasm32-wasip2
curl --data-binary "@target/wasm32-wasip2/debug/http_rust.wasm" localhost:8002/deploy/http-rust
curl -X POST -i http://127.0.0.1:8000/http-rust/echo -d "echo this!"
```