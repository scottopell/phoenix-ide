#!/usr/bin/env node
// HTTPS proxy for testing service workers locally

const https = require('https');
const http = require('http');
const fs = require('fs');
const path = require('path');

const BACKEND_PORT = process.env.BACKEND_PORT || 8000;
const HTTPS_PORT = process.env.HTTPS_PORT || 8443;

// Read certificates
const options = {
  key: fs.readFileSync(path.join(__dirname, 'certs', 'localhost.key')),
  cert: fs.readFileSync(path.join(__dirname, 'certs', 'localhost.crt'))
};

// Create HTTPS proxy server
https.createServer(options, (req, res) => {
  // Proxy to backend
  const proxyOptions = {
    hostname: 'localhost',
    port: BACKEND_PORT,
    path: req.url,
    method: req.method,
    headers: req.headers
  };

  const proxyReq = http.request(proxyOptions, (proxyRes) => {
    res.writeHead(proxyRes.statusCode, proxyRes.headers);
    proxyRes.pipe(res);
  });

  proxyReq.on('error', (err) => {
    console.error('Proxy error:', err);
    res.writeHead(502);
    res.end('Bad Gateway');
  });

  req.pipe(proxyReq);
}).listen(HTTPS_PORT, () => {
  console.log(`HTTPS proxy server running at https://localhost:${HTTPS_PORT}`);
  console.log(`Proxying to http://localhost:${BACKEND_PORT}`);
  console.log('\nNote: You may need to accept the self-signed certificate in your browser.');
});
