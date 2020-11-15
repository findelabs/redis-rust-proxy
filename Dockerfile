from rust:slim-stretch

RUN mkdir /app 

COPY entrypoint.sh /app/
COPY proxy-health.sh /app/proxy-health.sh

RUN chmod +x /app/entrypoint.sh /app/proxy-health.sh

RUN apt-get update && apt-get install -y redis-server
COPY Cargo.toml /app/Cargo.toml
COPY src /app/src
WORKDIR /app
RUN cargo install --path . --root /app/

#HEALTHCHECK --interval=10s --timeout=5s --retries=5 CMD /app/proxy-health.sh

ENTRYPOINT ["/app/entrypoint.sh"]

EXPOSE 6739
