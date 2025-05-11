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
- `GET /instances` - List all active upscaling instances
- `POST /instances` - Create a new upscaling instance
- `DELETE /instances/{uuid}` - Delete an instance
- `POST /instances/{uuid}/preload` - Preload upscaling parameters
- `POST /instances/{uuid}/upscale` - Upscale an image

## License

[MIT](LICENSE)
