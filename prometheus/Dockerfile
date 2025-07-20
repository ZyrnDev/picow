FROM prom/prometheus:latest AS prometheus

COPY ./prometheus.yml /etc/prometheus/prometheus.yml

ENTRYPOINT [ "/bin/prometheus" ]
CMD [ \
    "--config.file=/etc/prometheus/prometheus.yml", \
    "--storage.tsdb.path=/prometheus", \
    "--web.external-url=/prometheus", "--web.route-prefix=/prometheus" \
]

ARG GIT_COMMIT=unspecified
LABEL git_commit=$GIT_COMMIT