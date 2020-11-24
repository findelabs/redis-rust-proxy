## Setup

### Install docker-compose
sudo wget -O /usr/local/bin/docker-compose https://github.com/docker/compose/releases/download/1.27.4/docker-compose-Linux-x86_64 && sudo chmod +x /usr/local/bin/docker-compose

### Run docker-compose to create test cluster
sudo docker-compose -f examples/docker-compose.yml up --build --remove-orphans
