RVM is a service where you can upload guest services and execute them in a sandbox.

RVM uses a pooling allocatior with the following constraints:

* Limit of `268 KiB` of memeory per guests
* `100_000_000` of starting fuel for each guest.

Deploy: `curl --data-binary "@my-http-server.wasm" localhost:8002/deploy/my-http-server`
Invoke: `curl -X GET -i http://127.0.0.1:8000/my-http-server/secret`

# Host
Run `cargo run --release` to start the host.
It will listen for invokations on port `8000` and deployments on `8002`.
The host uses OpenDAL to store the modules, right the file system will be used, but it's easy to change service.

# Writing Guests for RVM
You can write guests in any language you want, as long as it compiles to webassembly.
This example uses python.

## Implement

Write your app and import the things you need.
There's an example in `guests/http_server.py` that implements a HTTP server that can be run in RVM.
Right now, the RVM expects all guests to be a HTTP proxy.
Every time it receives an `invoke` request it will run `IncomingHandler::handle` in your guest, with a forwarded HTTP request.

## Build
1. Make sure you have `componentize-py`, which can be installed via `pip install componentize-py`
2. `componentize-py -d ../wit -w rvm componentize http_server -o my-http-server.wasm`

## Deploy
`curl --data-binary "@my-http-server.wasm" localhost:8000/deploy/my-http-server` 

## Talk to your deployed app

`curl -X GET -i -H 'url: https://webassembly.github.io/spec/core/' http://127.0.0.1:8000/my-http-server/hash-all`
`curl -X POST -i http://127.0.0.1:8000/my-http-server/echo`
`curl -X GET -i http://127.0.0.1:8000/my-http-server/secret`

# Extending RVM

## Adding new host functions (i.e. functions that guests can call)
Edit the `host` import in `wit/world.wit`, which will be imported by the `rvm` world.
Edit the `rvm::lambda::host::Host` impl for `HostComponent` and add your newly added function.
The rust code will automatically run bindgen when compiled.

After you are done with the host canges, you need to make the changes available for the python guest by regenerating the bindings.
 1. Make sure you have `componentize-py`, which can be installed via:
    
     `pip install componentize-py`.
 2. Generate bindings to output dir `guests`:

    `componentize-py -d wit -w rvm bindings guests`
 3. Import your added function in your python code
    ```python
    from rvm.imports.host import (
        client_secret,
    )
    ```
4. Call your function! `secret = client_secret()`
   This will run the `client_secret` function from the host and return the response to your guest.

