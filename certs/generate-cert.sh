#!/bin/bash
# Generate self-signed certificate for localhost testing

openssl req -x509 -nodes -days 365 -newkey rsa:2048 \
  -keyout localhost.key \
  -out localhost.crt \
  -subj "/C=US/ST=Test/L=Test/O=Phoenix IDE/CN=localhost" \
  -extensions v3_ca \
  -config <(cat /etc/ssl/openssl.cnf \
    <(printf "\n[v3_ca]\nsubjectAltName=DNS:localhost,IP:127.0.0.1"))

echo "Generated localhost.crt and localhost.key"
