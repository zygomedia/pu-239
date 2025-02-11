# pu-239

<a href="https://crates.io/crates/pu-239"><img alt="Crate Info" src="https://img.shields.io/crates/v/pu-239.svg"/></a>

**pu-239** allows you to write server-side functions directly within your client-side code. It simplifies client-server communication by automating the serialization and transmission of function calls and their responses, as well as keeps relevant code closer together in an isomorphic applicaiton.    
Probably most useful for small projects or prototyping.

## Usage

Add `pu-239`, `postcard` and `anyhow` to your `Cargo.toml`:

```toml
[dependencies]
pu-239 = "*"
postcard = { version = "1", features = ["use-std"] }
anyhow = "1"
```

### Defining Server-Side Functions in Client Code

In your client code, annotate functions with `#[pu_239::server]` (do not import or rename the `pu_239::server` macro or the server end won't be able to find these). These functions will be replaced with a shim that serializes (with postcard) the arguments and sends to `crate::api::dispatch` (see below). It will have the same visibility as your server's `api` module, which is where the body will be eventually pasted.

```rust
#[pu_239::server]
pub async fn some_serverside_fn(arg: ArgType) -> ReturnType {
    use crate::some::server::module::Thing;
    
    Thing::do_something(arg).await
}

// ----- crate::api module -----


// totally free to swap out reqwest with any other http client
#[throws(anyhow::Error)]
pub async fn dispatch(serialized: Vec<u8>) -> impl std::ops::Deref<Target = [u8]> {
    static REQWEST_CLIENT: Lazy<reqwest::Client> = Lazy::new(reqwest::Client::new);

    REQWEST_CLIENT.post("localhost:8080/api")
        .body(serialized)
        .send().await?.error_for_status()?
        .bytes().await?
}
```

### Generating the Server API Dispatcher

On the server, route requests to a service of your choosing, then call `pu239::build_api!` to generate the `deserialize_api_match` function.

```rust
// e.g. with actix-web
actix_web::HttpServer::new(|| actix_web::App::new()
    // --- etc ---
    .service(web::resource("/api").to(api::api))
    // --- etc ---
)
    .run().await?;

// ----- crate::api module -----

pu_239::build_api!(["crates/client/src/lib.rs", "crates/other-client/src/lib.rs"]);

pub async fn api(req: web::Bytes) -> actix_web::HttpResponse {
    let mut bytes = ::actix_web::web::Buf::reader(req);
    match deserialize_api_match(&mut bytes).await {
        Ok(x) => actix_web::HttpResponse::Ok().body(x),
        Err(e) => actix_web::HttpResponse::InternalServerError().body(format!("{e:?}")),
    }
}
```

### Making sure server rebuilds when client code changes

For example, using [change-detection](https://crates.io/crates/change-detection), specify a `build.rs` for your server to track changes in the client code. If you don't do this - the `pu_239::build_api!` macro won't rerun if any of your client-defined serverside functions change.

```rust
fn main() {
    change_detection::ChangeDetection::path("../../client/src")
        .path("../../other-client/src")
        .generate();
}
```

### Calling Functions from Client Code

Call the server-side functions from your client code as if they were local `async` functions.

```rust
let result = some_serverside_fn(some_arg).await;
```

## How It Works

- The `#[pu_239::server]` macro on the client transforms the function into a stub that serializes the arguments and sends them to the server.
- On the server, `pu_239::build_api!` crawls client source code to find all `#[pu_239::server]` functions, copy-pastes their bodies (preserving module structure) and generates `deserialize_api_match`.

```rust
#[pu_239::server]
pub async fn some_serverside_fn(a: u64, b: i32) -> f32 {
    a as f32 + b as f32
}

// turns into

pub async fn some_serverside_fn(a: u64, b: i32) -> Result<f32, anyhow::Error> {
    const HASH: u64 = 18142343272683751701u64;

    let args = (a, b);
    let mut serialized = Vec::with_capacity(postcard::experimental::serialized_size(&HASH)? + postcard::experimental::serialized_size(&args)?);
    postcard::to_io(&HASH, &mut serialized)?;
    postcard::to_io(&args, &mut serialized)?;
    Ok(postcard::from_bytes(&crate::api::dispatch(serialized).await?)?)
}

// on the server
pub async fn some_serverside_fn(a: u64, b: i32) -> f32 {
    a as f32 + b as f32
}

async fn deserialize_api_match(mut bytes: impl std::io::Read) -> anyhow::Result<Vec<u8>> {
    let mut scratch = [0u8; 2048];
    let (hash, (mut bytes, _)) = postcard::from_io::<u64, _>((bytes, &mut scratch))?;
    match hash {
        18142343272683751701u64 => {
            let (a, b) = postcard::from_io::<_, _>((&mut bytes, &mut scratch))?.0;
            let res = some_serverside_fn(a, b).await;
            Ok(postcard::to_stdvec(&res)?)
        }
        method_id => Err(anyhow::anyhow!("Unknown method id: {method_id}"))
    }
}
```

## Limitations
- Compile errors in `#[pu_239::server]` will point at `pu239::build_api!` instead of the actual function
- Serverside functions in `include!("some/path/foo.rs")` or `#[path = "foo.rs"] mod c;` will not work
- Functions are distinguished by body hashes so changing any tokens in it will change the hash

## TODO:
- [ ] Remove dependency on `anyhow`
- [ ] Fix serverside fn compile error spans? not sure if possible
- [ ] Allow users to pick their own serialization library (not just postcard)
- [ ] Have actual examples that actually compile
- [ ] Macro parameters that can change client-side module/function path, server-side deserialize fn name, etc
- [ ] Alternative hashing strategies so server compiled with a different body but same signature can still match, at least sometimes?
