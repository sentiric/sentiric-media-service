FROM node:18-alpine
# YENİ SATIR: FFmpeg'i kuruyoruz.
RUN apk add --no-cache ffmpeg

WORKDIR /app
COPY package*.json ./
RUN npm install
COPY . .
EXPOSE 3003
EXPOSE 10000-10100/udp
CMD [ "npm", "start" ]