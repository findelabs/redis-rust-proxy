from rust:slim-stretch

RUN mkdir /app 

COPY entrypoint.sh /app/
COPY proxy-health.sh /app/proxy-health.sh

RUN chmod +x /app/entrypoint.sh /app/proxy-health.sh

RUN apt-get update && apt-get install -y redis-server
RUN cargo install --git https://github.com/findelabs/redis-rust-proxy.git --root /app/

HEALTHCHECK --interval=10s --timeout=5s --retries=5 CMD /app/proxy-health.sh

ENTRYPOINT ["/app/entrypoint.sh"]

EXPOSE 6739

CMD ["/app/bin/redis-rust-proxy"]
