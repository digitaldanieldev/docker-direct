# Docker-direct
Docker-direct is a simple tool that allows people to start and stop docker containers based on a simple allow-list. Docker-direct compares the containers that are hosted on the system with the containers listed in containers.txt and only allows users to directly control those docker containers.

## Why?
I built this so my kids can start and stop their own Minecraft servers without having to use tools like Portainer

## Usage:
`docker-direct [OPTIONS]`
`docker-direct -f listofminecraftservers -p 1235`

### Options:

**-f** --filename   Set the file that contains the list of allowed containers. <em>Default: containers.txt.</em>

The container names should be on seperate lines without any separators
`container-name-one`
	
	

**-p** --port       Set the port on which docker-direct should be accessible. <em>Default: 1234</em>

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

