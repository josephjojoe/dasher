# The benchmark image: dasher, Lightpanda and Chromium's headless shell side
# by side, with a hand-written Lambda runtime loop as the entrypoint.
# Lightpanda's release binary needs glibc 2.38+, which Lambda's managed
# runtimes do not have, hence Debian trixie.
FROM python:3.12-slim-trixie AS tools
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev curl ca-certificates
COPY runner.c /runner.c
RUN gcc -O2 -static -o /runner /runner.c
ARG LIGHTPANDA_URL=https://github.com/lightpanda-io/browser/releases/download/nightly/lightpanda-aarch64-linux
RUN curl -fsSL -o /lightpanda "$LIGHTPANDA_URL" && chmod +x /lightpanda

FROM python:3.12-slim-trixie
RUN apt-get update && apt-get install -y --no-install-recommends chromium-headless-shell fonts-liberation ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir boto3
ENV LAMBDA_TASK_ROOT=/var/task PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
COPY bootstrap.py handler.py cdp.py /var/task/
COPY bin/dasher /var/task/bin/dasher
COPY --from=tools /runner /lightpanda /var/task/bin/
WORKDIR /var/task
ENTRYPOINT ["python3", "/var/task/bootstrap.py"]
