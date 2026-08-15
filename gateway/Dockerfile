# Zero-dependency Node gateway. Build with the lockfile, install only prod deps
# (there are none — node:http + node:net only), then run.
FROM node:22-alpine

WORKDIR /app

COPY package.json package-lock.json ./
RUN npm ci --omit=dev

COPY handler.mjs server.mjs ./

ENV NODE_ENV=production
EXPOSE 8080

CMD ["node", "server.mjs"]
