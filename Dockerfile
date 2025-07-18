FROM node:18-alpine
WORKDIR /app
COPY package*.json ./
RUN npm install
COPY . .

# Hem API (3003) hem de RTP (10000-20000) portlarını açıyoruz.
EXPOSE 3003
EXPOSE 10000-20000/udp

CMD [ "npm", "start" ]