# Docker-direct

Docker-direct is a simple tool designed for managing Docker containers based on an allow-list. It allows users to start and stop only selected containers.

## Why?
I created Docker-direct to simplify the management of Docker containers, enabling my kids to start and stop their Minecraft servers without needing tools like Portainer.

## Usage

You can use Docker-direct with either of the following methods:

1. **Pass containers directly:** Use the `-c` option to specify a JSON array of container names directly on the command line.

2. **Specify a file:** Use the `-f` option to provide a file (default: `containers.txt`) listing allowed containers, each on a separate line.

Run Docker-direct using the following syntax:

`docker-direct [OPTIONS]`

Example:

`docker-direct -p 1234 -c '["minecraft-server-1.21-vanilla", "minecraft-server-1.16.5-modded"]' -l error`

Run Docker-direct on your server that hosts Docker containers.

Access Docker-direct via `http://<ip>:<port>/containers` in your web browser.

If something doesn't work as expected, check `docker-direct.log`.

### Options:

**-c --containers**  
Specify containers in JSON format directly via the command line. Example: `-c '["minecraft-server-1.21-vanilla", "minecraft-server-1.16.5-modded"]'`.

**-f --file**   
Specify the file containing the list of allowed containers. Each container name should be on a separate line without any separators. *Default: `containers.txt`.*

**-l --log**        
Specify the log level. Choose between info, debug and error. *Default: `info`.*

**-p --port**    
Set the port number for accessing Docker-direct. *Default: `1234`.*

**-h --help**       
Display help information.

**-v --version**    
Display version information.

## Assumptions
- Operating system: Linux
- Containers managed by Docker-direct must be pre-built. Start them using `docker run` or `docker compose`.

## Check Docker Service
Ensure Docker service is running:

`sudo systemctl status docker`

## Check Docker Daemon Socket
Verify Docker daemon connectivity:

`curl --unix-socket /var/run/docker.sock http://localhost/_ping; echo`

## Rust Installation and Cargo Build Instructions
Docker-direct is written in Rust. If you want to build it yourself, you need to have Rust and Cargo installed on your system.

### Installing Rust and Cargo
To install Rust and Cargo, you can use the official Rust installation script, `rustup`.

Run the following command in a terminal to install Rust:

`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

After installation, configure your current shell to use Rust:

`source $HOME/.cargo/env`

Verify the installation by checking the version of Rust and Cargo:

`rustc --version`
`cargo --version`

### Building Docker-direct with Cargo
Navigate to the directory containing the Docker-direct source code.

Run the following command to build the project:

`cargo build --release`

After the build process completes, the compiled binary will be located in the `target/release` directory. You can run Docker-direct using this binary:

`./docker-direct [OPTIONS]`

## API endpoints
To automate Docker container operations using Docker-direct, use the following API endpoints:

- Start container: `http://<ip>:<port>/containers/start?name=<container_name>`
- Stop container: `http://<ip>:<port>/containers/stop?name=<container_name>`

## Basic Security
Each API request in Docker-direct checks if the container being started or stopped is on the allow-list.

## Automated start of service using Systemd
Create docker-direct.service in /etc/systemd/system/ and start/enable
```
[Unit]
Description=docker-direct for container management
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/home/username/docker-direct -p 1234 -c '["minecraft-server-1.21-vanilla", "minecraft-server-1.16.5-modded"]' -l info
Type=simple
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```
