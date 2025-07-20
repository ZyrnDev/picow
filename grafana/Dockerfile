FROM grafana/grafana:latest AS grafana

COPY ./grafana.ini /etc/grafana/grafana.ini
COPY ./datasources.yml /etc/grafana/provisioning/datasources/datasources.yml
COPY ./dashboards /etc/grafana/provisioning/dashboards

ARG GIT_COMMIT=unspecified
LABEL git_commit=$GIT_COMMIT