# Docker-direct
Docker-direct is a simple tool that allows the starting and stopping docker containers based on an allow-list. Docker-direct compares the containers that are hosted on the system with the containers listed in containers.txt and only allows users to directly control those docker containers.

## Why?
I built this so my kids can start and stop their own Minecraft servers without having to use tools like Portainer.

## Usage:
`docker-direct [OPTIONS]`
`docker-direct -f listofminecraftservers -p 1235`

Rdun docker-direct on the server that hosts your docker containers.

Access docker-direct using `ip:port/containers` in your browser.

### Options:

**-a** --allowed   Set the file that contains the list of allowed containers. The container names should be on seperate lines without any separators. <em>Default: containers.txt.</em>


**-p** --port       Set the port on which docker-direct should be accessible. <em>Default: 1234</em>

**-l** --log        Set the log file name and location. <em>Default: docker-direct.log</em>

**-h** --help       Print help

**-v** --version    Print version


# assumption
You are using linux.
The containers that can be managed using docker-direct have to be built. So first start the containers using docker run or docker compose. 

Check docker service:
sudo systemctl status docker

Check docker daemon socket: 
curl --unix-socket /var/run/docker.sock  http://localhost/_ping; echo


# API
If you want to automate something using docker-direct, the API endpoints to start/stop containers are:
http://<ip>:<port>/containers/start?name=<name>
http://<ip>:<port>/containers/stop?name=<name>

## basic security
A check is done for each API request to see if the container that is being started/stopped is on the allow-list.

