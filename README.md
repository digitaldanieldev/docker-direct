# Docker-direct
Docker-direct is a simple tool designed for managing Docker containers based on an allow-list. It allows users to start and stop only those containers listed in containers.txt.

## Why?
I created Docker-direct to simplify the management of Docker containers, enabling my kids to start and stop their Minecraft servers without needing complex tools like Portainer.

## Usage:
Download the binary found in the repository under 'releases'. Create a file called containers.txt and specify the containers by name on separate lines.

`docker-direct [OPTIONS]`
`docker-direct -a listofminecraftservers -p 1235 `

Run Docker-direct on your server that hosts Docker containers.

Access Docker-direct via ip:port/containers in your web browser.

### Options:

**-a --allowed**   
Specify the file containing the list of allowed containers. Each container name should be on a separate line without any separators. <em>Default: containers.txt.</em>


**-p --port**    
Set the port number for accessing Docker-direct. <em>Default: 1234</em>

**-l --log**        
Specify the log file name and location. <em>Default: docker-direct.log</em>

**-h --help**       
Display help information.

**-v --version**    
Display version information.

# Assumptions:
- Operating system: Linux
- Containers managed by Docker-direct must be pre-built. Start them using `docker run` or `docker compose`.

## Check Docker service:
Ensure Docker service is running:

`sudo systemctl status docker`

## Check docker daemon socket: 
Verify Docker daemon connectivity:

`curl --unix-socket /var/run/docker.sock  http://localhost/_ping; echo`

# Rust Installation and Cargo Build Instructions
Docker-direct is written in Rust. If you want to build it yourself, you need to have Rust and Cargo installed on your system. 

##  Installing Rust and Cargo
To install Rust and Cargo, you can use the official Rust installation script, `rustup`.

Run the following command in a terminal to install Rust:
`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

After installation, configure your current shell to use Rust.
`source $HOME/.cargo/env`

Verify the installation by checking the version of Rust and Cargo.
`rustc --version`
`cargo --version`

## Building Docker-direct with Cargo
Navigate to the directory containing the Docker-direct source code.

Run the following command to build the project:
`cargo build --release`

After the build process completes, the compiled binary will be located in the target/release directory. You can run Docker-direct using this binary.

`./docker-direct [OPTIONS]`

# API
To automate Docker container operations using Docker-direct, use the following API endpoints:
- Start container: `http://ip:port/containers/start?name=name`
- Stop container: `http://ip:port/containers/stop?name=name`

## Basic security
Each API request in Docker-direct checks if the container being started or stopped is on the allow-list.
