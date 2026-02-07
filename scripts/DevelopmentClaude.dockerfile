ARG TAG=local
FROM nebulastream/nes-development:${TAG}

RUN (curl -fsSL https://claude.ai/install.sh | bash) && /home/ls/.local/bin/claude --version
