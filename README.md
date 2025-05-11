# Upsclr Server

A server for upscaling images using various plugins.

## Features

- Load plugins for different upscaling algorithms
- Process images via HTTP API
- Manage upscaling instances

## Build

```sh
cargo build --release
```

The compiled binary will be in `target/release/upsclr-server`.

## Usage

```sh
upsclr-server [OPTIONS]
```

### Command-line Options

```text
Options:
      --host [<HOST>]              Hostname/IP to bind the server to. If this option is specified without value, it will default to "*", meaning the server will listen on all interfaces [env: UPSCLR_SERVER_HOST=] [default: localhost]
  -p, --port <PORT>                Port number to listen on [env: UPSCLR_SERVER_PORT=] [default: 6795]
      --plugins-dir <PLUGINS_DIR>  Directory containing plugin shared libraries [env: UPSCLR_SERVER_PLUGINS_DIR=] [default: plugins]
  -h, --help                       Print help
  -V, --version                    Print version
```

### Configuration Options

The server can be configured using command-line arguments or environment variables:

#### Host Configuration

- **Description:** Specifies the network interface to bind to
- **Command-line:** `--host` or `--host <HOST>`
- **Environment Variable:** `UPSCLR_SERVER_HOST`
- **Default Value:** `localhost` if not specified, `*` (all interfaces) if `--host` is specified without a value
- **Example Value:** `0.0.0.0` (all interfaces)

#### Port Configuration

- **Description:** Specifies the port number to listen on
- **Command-line:** `-p, --port <PORT>`
- **Environment Variable:** `UPSCLR_SERVER_PORT`
- **Default Value:** `6795`
- **Example Value:** `8080`

#### Plugins Directory

- **Description:** Directory containing plugin shared libraries
- **Command-line:** `--plugins-dir <PLUGINS_DIR>`
- **Environment Variable:** `UPSCLR_SERVER_PLUGINS_DIR`
- **Default Value:** `plugins`
- **Example Value:** `/usr/lib/upsclr/plugins`

### Examples

Start the server with default settings (localhost:6795):

```sh
upsclr-server
```

Expose the server to all network interfaces on port 8080:

```sh
upsclr-server --host 0.0.0.0 --port 8080
```

Expose the server to all network interfaces with default port:

```sh
upsclr-server --host
```

Using environment variables (PowerShell):

```powershell
$env:UPSCLR_SERVER_HOST="0.0.0.0"
$env:UPSCLR_SERVER_PORT="8080"
$env:UPSCLR_SERVER_PLUGINS_DIR="/path/to/plugins"
upsclr-server
```

Using environment variables (Bash):

```bash
UPSCLR_SERVER_HOST=0.0.0.0 UPSCLR_SERVER_PORT=8080 UPSCLR_SERVER_PLUGINS_DIR=/path/to/plugins upsclr-server
```

## API Endpoints

- `GET /plugins` - List all available plugins
  - Response body (JSON):
    - `[].id`: The ID of the plugin
    - `[].name`: The name of the plugin
    - `[].version`: The version of the plugin
    - `[].description`: The description of the plugin
    - `[].engines[].name`: The name of the engine
    - `[].engines[].description`: The description of the engine
    - `[].engines[].version`: The version of the engine
    - `[].engines[].config_schema`: The configuration schema for the engine (JSON schema)
    - `[].engines[].plugin_id`: The ID of the plugin (same as `[].id`)
    - `[].engines[].engine_index_in_plugin`: The index of the engine in the plugin
- `GET /instances` - List all active upscaling engine instances
  - Response body (JSON):
    - `[].uuid`: The UUID of the instance
    - `[].plugin_id`: The ID of the plugin
    - `[].plugin_name`: The name of the plugin
    - `[].engine_name`: The name of the engine
- `POST /instances` - Create a new upscaling engine instance
  - Query parameters:
    - `dry_run` (optional): Set to `1` or `true` to test the creation of an instance without actually creating it
  - Request body (JSON):
    - `plugin_id`: The plugin that contains the upscaling engine
    - `engine_name`: The engine to use for upscaling
    - `config`: The configuration for the engine
  - Response body (JSON):
    - `instance_id`: The UUID of the created instance. `null` if `dry_run` is set
    - `validation.is_valid`: `true` if the configuration is valid, `false` otherwise
    - `validation.error_count`: The number of errors if the configuration is invalid
    - `validation.error_messages`: A list of errors if the configuration is invalid
    - `validation.warning_count`: The number of warnings
    - `validation.warning_messages`: A list of warnings
- `DELETE /instances/{uuid}` - Delete an engine instance
  - No request body
  - No response body
- `POST /instances/{uuid}/preload?scale=N` - Preload upscaling resources
  - Query parameters:
    - `scale` (required): The scale factor for the upscaling
  - No request body
  - No response body
- `POST /instances/{uuid}/upscale?scale=N` - Upscale an image
  - Query parameters:
    - `scale` (required): The scale factor for the upscaling
  - Request headers:
    - `Accept` (required): `image/png`, `image/jpeg`, or `image/x-raw-bitmap`
      - The `image/x-raw-bitmap` is our custom type for raw bitmap images with 16-byte header
  - Request body (Multipart form-data):
    - `file` (required): The image file to be upscaled. Currently supports `image/png`, `image/jpeg`, `image/webp`, and `image/x-raw-bitmap`.
  - Response body (image data):
    - The upscaled image in the requested format

## License

[MIT](LICENSE)
