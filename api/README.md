# API propia para RustDesk

Primera implementación compatible con las funciones de cuenta y libreta personal
del cliente oficial de RustDesk.

Incluye una consola web responsive para crear usuarios, suspender cuentas,
cambiar contraseñas y revisar métricas básicas.

- Bienvenida: `https://api.lvltechmx.com/`
- Acceso: `https://api.lvltechmx.com/login`
- Administración: `https://api.lvltechmx.com/admin`

## Endpoints

- `GET /health`
- `GET /api/login-options`
- `POST /api/login`
- `POST /api/currentUser`
- `POST /api/logout`
- `GET /api/ab`
- `POST /api/ab`
- `POST /api/ab/personal` responde `404` para activar el modo de libreta legacy
- `/admin/api/*` requiere una cuenta con permisos administrativos

## Ejecución

```bash
cp .env.example .env
# Editar API_ADMIN_PASSWORD antes de iniciar.
docker compose -f compose.yml up -d --build
```

Los datos quedan en `./data/api.sqlite3`.
