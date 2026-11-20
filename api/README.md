# API propia para RustDesk

Primera implementación compatible con las funciones de cuenta y libreta personal
del cliente oficial de RustDesk.

## Endpoints

- `GET /health`
- `GET /api/login-options`
- `POST /api/login`
- `POST /api/currentUser`
- `POST /api/logout`
- `GET /api/ab`
- `POST /api/ab`
- `POST /api/ab/personal` responde `404` para activar el modo de libreta legacy

## Ejecución

```bash
cp .env.example .env
# Editar API_ADMIN_PASSWORD antes de iniciar.
docker compose -f compose.yml up -d --build
```

Los datos quedan en `./data/api.sqlite3`.
