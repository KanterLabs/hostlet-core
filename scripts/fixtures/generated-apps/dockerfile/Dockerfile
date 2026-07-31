FROM node:22-alpine
WORKDIR /app
COPY --chown=node:node package.json server.js ./
USER node
CMD ["node", "server.js"]
