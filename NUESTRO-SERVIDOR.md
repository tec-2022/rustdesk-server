# Servidor RustDesk propio

Este repositorio es nuestra copia mantenible del servidor libre de RustDesk. El
VPS compila su imagen directamente desde este codigo; no descarga la imagen de
RustDesk, no requiere acceso a un registro privado y no usa scripts de
instalacion de terceros. GitHub Actions compila y publica la misma imagen como
verificacion independiente de cada cambio.

## Preparar el VPS

Requisitos: Linux `amd64`, Docker Engine y el complemento Docker Compose.

```bash
git clone https://github.com/tec-2022/rustdesk-server.git
cd rustdesk-server
nano .env
docker compose build
docker compose up -d
docker compose ps
```

En `.env`, conserva `DATABASE_URL` y agrega `RUSTDESK_HOST` con el dominio o IP
publica del VPS. Por ejemplo:

```dotenv
DATABASE_URL=sqlite://./db_v2.sqlite3
RUSTDESK_HOST=rustdesk.example.com
```

No incluyas `https://` ni un puerto en `RUSTDESK_HOST`.

Abre en el firewall del proveedor y del sistema:

- TCP: `21115`, `21116`, `21117`, `21118`, `21119`
- UDP: `21116`

La clave publica que se configura en los clientes queda en `data/id_ed25519.pub`:

```bash
cat data/id_ed25519.pub
```

En cada cliente RustDesk configura:

- Servidor ID: el valor de `RUSTDESK_HOST`
- Servidor relay: el valor de `RUSTDESK_HOST`
- Clave: el contenido de `data/id_ed25519.pub`

## Actualizar

```bash
git pull --ff-only
docker compose build
docker compose up -d
```

Los datos persistentes y las claves permanecen en `./data`. Haz respaldo de
esa carpeta; perder `id_ed25519` obliga a distribuir una nueva clave a todos
los clientes.

## Relacion con el proyecto original

El codigo original procede de `rustdesk/rustdesk-server` y conserva su licencia
AGPL-3.0. Nuestro remoto `upstream` permite revisar e integrar actualizaciones
de seguridad sin ceder el control de nuestra imagen, configuracion o VPS.
