ARG TAG=local
FROM nebulastream/nes-development:${TAG}

ENV PATH="/home/ls/.local/bin:${PATH}"
RUN (curl -fsSL https://claude.ai/install.sh | bash) && claude install
