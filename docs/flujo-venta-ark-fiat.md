# Flujo de venta Ark + fiat con escrow VHTLC

> Diseño de alto nivel del port del `EscrowBackend` de Mostro desde
> Lightning (hold invoice) a Arkade VHTLC. Ninguna línea de código se
> implementa aquí; este documento fija las premisas y la mecánica antes
> del trabajo de port.

## 1. Objetivo

Operar un marketplace P2P estilo Mostro (matchmaker no-custodial sobre
Nostr) donde el asentamiento use **vtxos VHTLC de Arkade** en lugar de
hold invoices Lightning. La comunidad compra/vende con un escrow donde
el **vendedor no puede hacer trampa**: el dinero queda congelado en un
script VHTLC y nadie — ni vendedor ni comprador — puede moverlo sin la
colaboración del **daemon (rol `server`)**.

## 2. Modelo comunitario (contexto)

- **Onboarding onchain** > 500 sats.
- **Pagos ark → ark** mínimo 1 sat (asentamiento interno del escrow).
- **Salida a Lightning** vía solver oficial (mín 500 sats, rango
  500–50.000).
- El **ASP no cambia**: se reutiliza un ASP existente (p.ej.
  `mutinynet.arkade.sh`). No se monta uno.

## 3. Concepto de bloqueo en Ark (vs. una 2-de-3 clásica)

El escrow **no es una multifirma 2-de-3 libre sobre UTXOs onchain**.
El vendedor **funda un vtxo desde su propia wallet Arkade** a un script
VHTLC anclado en un UTXO conjunto del ASP. Los sats quedan congelados
en el script hasta que se gasta una de sus hojas.

El script VHTLC declara tres roles:

| Rol    | Partido                | Notas                                  |
|--------|------------------------|----------------------------------------|
| sender | Vendedor (funda el vtxo) | El que pone los sats en el escrow       |
| receiver | Comprador             | Destinatario del `claim`                |
| server | **Daemon Mostro (fork)** | Guarda el preimage `R`; co-firma cada hoja |

La garantía anti-trampa **no** depende de combinar firmas a voluntad
(no es un multisig genérico). Es estructural:

- La hoja `claim` **requiere el preimage correcto** más la firma
  `server`. Solo el `server` conoce `R`, así que el vendedor no puede
  armar un claim a favor del comprador (ni a favor propio).
- Las hojas `refund` / `refundWithoutReceiver` **también requieren la
  firma `server`**. El vendedor no puede “devolverse” los sats solo.
- Por tanto, **el daemon (server) es el árbitro**: sin su firma nadie
  mueve los fondos bloqueados.

### Hojas de gasto (del SDK `@arkade-os/swap`)

| Hoja                   | Firmantes                          | Precondición                          | Destino          |
|------------------------|------------------------------------|---------------------------------------|------------------|
| `claim`                | receiver + **server**              | preimage `R` correcto                 | comprador        |
| `refund`               | sender + receiver + **server**     | tras `refundLocktime`                 | vendedor         |
| `refundWithoutReceiver`| sender + **server**                | tras CLTV del `refundLocktime`        | vendedor         |

Recursos del SDK relevantes:
`VHTLC.ScriptV2`, `VHTLCV2ContractParams` (`sender`, `receiver`,
`server`, `preimageHash`, `refundLocktime`, delays CSV),
`SWAP_LOCKUP_CONTRACT_TYPE = "vhtlc-v2"`,
`buildHtlcClaim` / `buildHtlcRefund`,
`pushRefundWithoutReceiver`, `newPreimage`, `paymentHashOf`.

## 4. Propiedad del preimage y veto

Decisión confirmada e innegociable: **el daemon guarda el preimage `R`**,
como hacía Mostro-Lightning. Consecuencias:

- El `server` es el único que conoce `R` entre los roles.
- El vendedor **no puede** cobrar ni armar un claim.
- El comprador no puede cobrar hasta que el server firme `claim`
  (que solo ocurre tras `release` del vendedor o `admin-settle`).

Esto reproduce el modelo “2-de-3 con árbitro” de Mostro: hay tres
firmantes en el script, pero la llave de decisión es la del server.

## 5. Rol `server`: wallet Ark del daemon

El daemon debe poseer una **wallet Arkade propia** (clave + conexión a
`arkd`) para co-firmar los VHTLC como `server`. Sin esa segunda firma,
ni `claim` ni `refund` ni `refundWithoutReceiver` son gastables. Es un
requisito de infraestructura nuevo, distinto del LND actual:

- Config nueva (ej. `arkd` URL + clave `server`).
- Co-firma en el flujo de `release` / `admin-settle` (`claim`).
- Co-firma en `cancel` / `admin-cancel` / expiry (`refund*`).

Garantía operativa: el server **solo** firma `claim` cuando hay
`release` del vendedor o `admin-settle`; en cualquier otro escenario
que no resuelva la disputa, no firma.

## 5.1 Billetera Ark del escrow (destino de la tarifa)

La tarifa de **0.5% (0.25% aportado por el vendedor + 0.25% aportado por
el comprador)** es **para el escrow** y se abona a una **billetera Ark
dedicada del escrow**, **separada** de la del server (daemon).

| Billetera | Rol | Recibe |
|-----------|-----|--------|
| Billetera del server (daemon) | Co-firma los VHTLC (`claim`/`refund*`) | No recibe tarifa por transacción |
| **Billetera del escrow** | Cobra los fees 0.25%+0.25% | La fracción de tarifa del vtxo en cada `claim` |

Consecuencias en el asentamiento:
- Al hacerse `claim`, el vtxo escrow se reparte entre el **comprador
  (neto)** y la **billetera del escrow (tarifa)**.
- La billetera del escrow es una wallet Arkade aparte (clave +
  conexión a `arkd`), dedicada solo a recaudar los fees; no co-firma
  contratos.

## 6. Dos timers distintos (no confundir)

### 6.1 `expiration_hours` — oferta sin tomar

Una oferta **no tomada** desaparece a las **24h** (`expiration_hours = 24`).
No hay escrow todavía (decisión del §2): si no se toma, simplemente se
retira del libro. No interviene ningún `refundLocktime`.

### 6.2 `refundLocktime` — escrow de una operación iniciada

Una vez la oferta es **tomada** y el vendedor **funda el VHTLC**, el
escrow corre su propia línea de tiempo (`refundLocktime`). El valor se
**fija al fundar el vtxo** (como el CLTV de la hold invoice) y **no se
extiende en vuelo**.

**Configuración del refundLocktime:** hereda de
`hold_invoice_cltv_delta` del `settings.toml` del nodo. El
administrador del Mostro define cuántos bloques dura el escrow (default
144 bloques ≈ 24h; algunos nodos lo ponen hasta 432 bloques ≈ 3 días).
En Ark, el mismo valor se usa como `refundLocktime` del VHTLC.

**Diferencias clave frente a LND:**

- En LND, la hold invoice vencida se **auto-resuelve sola** (LND la
  cancela y refundea al vendedor sin intervención).
- En Ark, **ninguna hoja se ejecuta sin firmas** — el vencimiento del
  `refundLocktime` NO devuelve los sats al vendedor por sí solo.

| Hoja | Firmantes | ¿Requiere server tras vencerse? |
|------|-----------|----------------------------------|
| `claim` | receiver + **server** | Sí (server + preimage) |
| `refund` | sender + receiver + **server** | Sí |
| `refundWithoutReceiver` | sender + **server** | Sí (server, tras CLTV) |

La firma del `server` (**el escrow**) es **necesaria en los tres caminos
incluso después de `refundLocktime`**. El vencimiento no "libera al
vendedor solo"; solo **madura las hojas de refund**:

- **Antes** de `refundLocktime`: solo `claim` (a favor del comprador) es
  el camino razonable; `refund` no está maduro.
- **Después** de `refundLocktime`: `refund` (y `refundWithoutReceiver`
  tras CLTV) quedan **gastables**, pero el vendedor **no puede usarlas
  sin la firma server**.

**Consecuencia honesta:** el server que queda inactivo tras el vencimiento
no desbloquea al vendedor; deja los sats **congelados en el script** hasta
que alguien co-firme. Por eso `refundLocktime` NO se piensa como "se
devuelve solo al vendedor", sino como "**pasado el instante, el server
queda legitimado/obligado por política a firmar refund a favor del
vendedor** si no hubo resolución". Y el comprador debe abrir la disputa
**antes** (§6.3), porque si no, el server — por política de emergencia —
firmará refund al vendedor (§11.2).

### 6.3 Por qué es una línea roja: el escrow no sabe si el fiat se movió

El escrow **no tiene forma de saber con certeza que el fiat se envió**.
La única señal es la **notificación `fiat-sent` del comprador**; no hay
verificación on-chain del flujo fiat. Por eso:

- **Cualquier parte puede abrir disputa** (como en el protocolo oficial
  de Mostro). El vendedor también puede abrirla si necesita proteger su
  escrow (ej. si el comprador no envió fiat y el vendedor quiere cancelar
  antes de que expire).
- Si el comprador ha enviado el fiat y el vendedor **no libera**, el
  comprador debe **abrir la disputa ANTES de `refundLocktime`**; de lo
  contrario, al vencer, **gana el vendedor** on-chain (refund).
- **Carga de la prueba en una disputa**:
  - **Comprador**: aporta **todas las pruebas posibles de que sí envió**
    el fiat (comprobante, tx, identidad de la cuenta, etc.).
  - **Vendedor**: aporta **pruebas de que no recibió** el dinero, o
    pruebas de que el comprador no envió fiat.

### 6.4 Rol del `scheduler` (reloj / avisador, no árbitro)

Un **job `scheduler`** vigila las operaciones en discontinuidad (sobre
todo la cercanía a `refundLocktime`):

- Advierte/escala cuándo un escrow se acerca a `refundLocktime` sin
  disputa ni resolución, de modo que **el comprador abra la disputa** o
  el **solver humano resuelva** a tiempo.
- **Nunca arbitra por sí mismo**; el arbitraje lo hace un **solver
  humano** (ver §11). El scheduler es un reloj / avisador, no un árbitro.

## 7. Flujo de venta deriva en escrow VHTLC

```text
VENDEDOR                       DAEMON (server, guarda R)          COMPRADOR
   |  crea oferta kind 38383      |                                 |
   |------------------------------->                                 |
   |                               |    take de la oferta (buyer)     |
   |                               <----------------------------------|
   |  funda vtxo VHTLC (escrow)    |                                 |
   |  sender=Vendedor              |                                 |
   |  receiver=Comprador           |                                 |
   |  preimageHash=H(R)            |                                 |
   |  (el daemon conoce R)         |                                 |
   |===============================>|                                 |
   |  pago fiat             ...pago fiat...                          |
   |                               |                                 |
   |  release (autoriza)           |                                 |
   |-------------------------------------->                          |
   |                               |  firma claim (server + R)       |
   |                               |---------- claim --------------> |  cobra neto
```

- **Vendedor funda** el vtxo VHTLC con `preimageHash`, dejando sus sats
  congelados en el script. No hay custodia del matchmaker: los fondos
  quedan en “la wallet del vendedor” pero **bloqueados** en el VHTLC.

### Secuencia de firmas en el flujo normal (sin disputa)

En el flujo feliz, la liberación ocurre en dos partes que NO se
confunden:

1. **`release` del vendedor = autorización, no la firma on-chain.**
   El vendedor (sender) emite la acción `release` para **autorizar** que
   el escrow mueva el dinero. Por sí solo no gasta nada.
2. **Co-firma `claim` del escrow = el acto que mueve los sats.**
   Al recibir un `release` válido, el **daemon (server)** firma la hoja
   `claim` con su clave `server` + el **preimage `R`**. Es esta firma del
   server la que **libera los sats al comprador** (receiver), que además
   notariza como destino.

La hoja `claim` del VHTLC exige `receiver` (comprador) + **server** y se
desbloquea con el preimage `R` (que solo guarda el server). Por tanto:

- Sin el `release` del vendedor, el server **no firma** el `claim`.
- Sin la firma del server, el `release` del vendedor **es solo una
  intención sin efecto** — el dinero no se mueve.

Es el equivalente exacto de Mostro-Lightning: allí el `release` hacía que
el daemon **settlée** la hold invoice a favor del comprador y pagara
(`do_payment`); en Ark, el `release` hace que el **server co-firme
`claim`** → los sats van directo al comprador.

### Orquestador del fundado (decisión)

El **fundado se dispara en la wallet del vendedor (sender)**, no en el
daemon. El reparto de responsabilidades:

| Actor | Responsabilidad |
|-------|-----------------|
| Wallet del vendedor | Gatilla el fundado del vtxo VHTLC contra el ASP (genera/usa `R`, construye el script con `buildHtlcClaim`, `register_inputs`/fund, confirma que el vtxo se ancló). Igual que hoy hace un swap ark→lightning. |
| Daemon (server)     | **Aporta el `preimageHash`** cuando el vendedor declara que va a fundar, y **vigila + confirma** que el vtxo aparezca fundado para activar el escrow (sustituye al `invoice_subscribe`). |

Razón: mantener la no-custodia. El daemon no firma ni dispara la
fundada por la wallet del vendedor; solo entrega el `preimageHash` y
observa el estado del vtxo. El orquestador criptográfico del fundado es
la **wallet del vendedor**.
- **Pago fiat** fuera de cadena (relación comercial de la comunidad).
- **`release`**: el vendedor firma; el daemon `server` firma `claim`;
  el vtxo escrow va **directo al comprador** (neto).
- **Fees**: 0.5% repartido **0.25% aportado por el vendedor + 0.25%
  aportado por el comprador**. Ambos son **para el escrow**, que tiene
  **su propia billetera Ark**, separada de la del server (daemon).
- **Disputas**: `admin-settle` (vuelve el dinero a favor del comprador
  vía claim) o `admin-cancel` / expiry (refund al vendedor), siempre
  **antes de `refundLocktime`**.

## 8. El payout al comprador NO es un segundo pago (clave de alcance)

En Mostro-Lightning el vendedor pagaba el hold invoice del buyer y el
daemon luego hacía un **segundo pago** (`do_payment`) para liberar el
invoice del buyer. Eso es un flujo de **dos saltos**.

En Ark/VHTLC el escrow **ES el payoff**: al hacer `claim`, el vtxo
escrow va directamente al comprador (receiver). **No existe un
`do_payment` / segundo salto que portar** — se elimina, no se adapta.
Esto es un alivio de alcance, no un costo extra.

Superficie remanente de LND que **sí** hay que reconciliar (no traslada
t:1):
- `do_payment` / `payment_success` / `reconcile_inflight_payout`: se
  retiran; el destino del buyer es el vtxo `receiver`, no una dirección
  de pago.
- `invoice_subscribe` (vigilar el hash de un hold invoice “pagado”): se
  sustituye por **observar el estado del vtxo VHTLC** (¿se fundó? ¿se
  claimeó?), un modelo de evento distinto.
- `payment_request` / `buyer_invoice` (destino de pago Lightning): se
  sustituye por la **dirección/vtxo Arkade** del comprador.

## 9. Equivalencia de timeouts de espera (15 minutos)

En Mostro-Lightning, cuando una oferta es tomada, hay un período de
**15 minutos** (`expiration_seconds = 900`) para que la parte
correspondiente complete una acción. Si no lo hace, la oferta se cancela
y se re-publica. Esto NO depende de Lightning; depende de **que una de
las partes no actuó a tiempo**.

### Estados de espera (Lightning → Ark)

| Estado | Acción esperada | Timeout | Uso en Ark |
|--------|-----------------|---------|------------|
| `WaitingPayment` | Vendedor funda el vtxo VHTLC | 15 min | **Reutilizado**: tras el take, Mostro crea el VHTLC y el vendedor recibe `PayInvoice` con el id del VHTLC como "payment request" |
| `WaitingBuyerInvoice` | Comprador envía su dirección Arkade (receiver) | 15 min | **Reutilizado**: `validate_buyer_invoice` en modo Ark no valida BOLT11; se guarda la dirección receiver |

**Decisión:** se **reutilizan los estados `WaitingPayment` / `WaitingBuyerInvoice`** en lugar de introducir `WaitingVhtlcFund` / `WaitingReceiverAddress`. Cero cambios en `mostro-core`, cero migraciones; el frontend no necesita estados nuevos.

### Flujos equivalentes

**Escenario A: Comprador toma oferta de venta (buy order)**
1. Comprador toma → `show_vhtlc_escrow_request` crea el VHTLC (clave = el
   "hash" guardado en la orden) y la pasa a `WaitingPayment`.
2. Vendedor recibe `PayInvoice` con el id del VHTLC y debe **fundar el vtxo**
   en 15 minutos (como pagar la hold invoice).
3. Si no lo hace → el refunder Ark cancela el VHTLC (`refund`),
   `cancel` + `republish` (misma lógica de `job_cancel_orders`).

**Escenario B: Vendedor toma oferta de compra (sell order)**
1. Vendedor toma → la orden pasa a `WaitingBuyerInvoice`.
2. Comprador debe **enviar su dirección Arkade** (receiver) en 15 minutos
   (como enviar la factura Lightning).
3. Si no lo hace → `cancel` + `republish`.

### Mapeo del scheduler

El core compartido `cancel_waiting_orders_tick` (en `src/scheduler.rs`)
maneja ambas variantes: el `job_cancel_orders` de Lightning se convierte en
su wrapper (`run_bonds = true`) y el nuevo `job_cancel_orders_arkade`
(`run_bonds = false`, sin bloques de bonds) arranca en el modo Arkade.

La mecánica es idéntica: un tick periódico revisa órdenes en estado de
espera, verifica si `taken_at` superó `expiration_seconds` (15 min), y si
es así, **cancela el VHTLC pendiente** (`cancel_hold_invoice` → refund del
sender) y re-publica la oferta.

## 10. Estrategia del port (adaptar el trait actual)

Se adapta el trait `EscrowBackend` en lugar de un refactor total,
manteniendo el contrato:

| Trait actual (LN)            | Comportamiento Ark                          |
|------------------------------|---------------------------------------------|
| `create_hold_invoice(desc, amount)` | Crear VHTLC con `preimageHash`, exponer camino/camino de fundado |
| `settle_hold_invoice(preimage)`    | Firma `claim` del vtxo a favor del receiver |
| `cancel_hold_invoice(hash)`        | `refundWithoutReceiver` / devolución        |
| `chain_height()`              | Auxiliar para el `refundLocktime`           |

Nuevo backend `ArkadeBackend` (rama `feature/vhtlc-escrow`), que sustituye
al `LndConnector` como escrow. El `CashuBackend` stub puede permanecer
como es; no es plantilla.

Handlers a tocar (mapeo preliminar):
- `src/util.rs`: `show_hold_invoice` (~:1572) e `invoice_subscribe`
  (~:1803), `settle`/`cancel` flows.
- `src/app/release.rs`, `src/app/admin_cancel.rs`, `src/app/admin_settle.rs`,
  `src/app/admin_take_dispute.rs`, `src/app/fiat_sent.rs`.
- `src/app/dispute.rs` (ciclo de disputa a conservar: notifica a ambos
  peers, publica kind 38386, alerta al solver humano).
- `src/scheduler.rs` (vigila el `refundLocktime`; alerta/escala al solver;
  política de emergencia, no arbitra).
- `src/db.rs` (persistencia del VHTLC, `preimageHash`, `refundLocktime`,
  estado de disputa).
- Retirar `do_payment` / `reconcile_inflight_payout`.

## 11. Disputas y vencimiento (ciclo de Mostro-Lightning)

### 10.1 Ciclo de disputa (solver humano)

Se **sigue lo más fielmente posible el ciclo de Mostro-Lightning**. La
disputa la resuelve un **solver humano**, no el daemon ni un autómata.

1. **Cualquier parte puede abrir la disputa** (como en el protocolo
   oficial de Mostro): el comprador (si envió fiat y el vendedor no
   libera) o el vendedor (si necesita proteger su escrow). Emite la
   acción **`dispute`** (`src/app/dispute.rs`). El daemon valida el
   estado (Active/FiatSent), crea el registro `Dispute` (NIP-33 kind
   38386), **notifica a ambos peers** y **publica el evento de disputa
   en Nostr** — esto alerta al solver humano.
2. El **solver humano** toma la disputa y **recopila las pruebas de ambos
   peers** (revisa el historial de mensajes/eventos del orden en Nostr y
   la evidencia que cada parte aporta):
   - **Comprador**: aporta **todas las pruebas posibles de que sí envió**
     el fiat (comprobante, tx, identidad de la cuenta, etc.).
   - **Vendedor**: aporta **pruebas de que no recibió** el dinero, o
     pruebas de que el comprador no envió fiat.
3. Tras revisar las pruebas, el solver resuelve con un comando admin:
   - **`admin-settle`** → el daemon co-firma `claim` (con `R`); el vtxo
     va al comprador (neto) + tarifa al escrow.
   - **`admin-cancel`** → el daemon co-firma `refund` /
     `refundWithoutReceiver`; devuelve al vendedor.
   - Acción `take_dispute`/`admin-take-dispute` para que el solver adopte
     la gestión (`src/app/admin_take_dispute.rs`).
4. El daemon cierra el `Dispute` y publica el evento actualizado
   (`close_dispute_after_user_resolution`, `src/app/dispute.rs`), igual
   que en Mostro-Lightning.

### 10.2 Rol del `scheduler` (reloj, no árbitro)

El `scheduler` **no resuelve disputas**. Su único rol frente al
`refundLocktime`:

- Vigila periódicamente las operaciones en discontinuidad.
- Si un escrow está **cerca de `refundLocktime`** sin disputa, **advierte
  a las partes** de que deben **abrir la disputa** (y escala al solver
  humano si ya hay una, para que resuelva dentro de la ventana).
- Si la ventana se agota sin disputa ni resolución, el **server firma
  refund a favor del vendedor por política de emergencia** — los sats
  no se devuelven solos (§6.2), pero el server queda legitimado a
  co-firmar el refund para despejar el escrow (§6.3).
- Es un **reloj / avisador**: nunca arbitra. La decisión la toma el
  **solver humano**.

### 10.3 Tabla de resolución

| Escenario                | Quién abre/decide | Acción server                              | Resultado              |
|--------------------------|-------------------|--------------------------------------------|------------------------|
| `release` del vendedor   | Vendedor          | firma `claim` (con `R`)                     | comprador neto + escrow fee |
| Disputa (cualquier parte) | **Buyer o Seller** | — | — |
| `admin-settle` (disputa) | **Solver humano** | firma `claim` (con `R`)                     | va al comprador        |
| `admin-cancel` (disputa) | **Solver humano** | firma `refund`/`refundWithoutReceiver`      | devuelve al vendedor  |
| `refundLocktime` venció sin disputa | **Política de emergencia** (server) | firma `refund`/`refundWithoutReceiver` al vendedor | **vuelve al vendedor** (vía server, no automático) |
| disputa abierta, cerca del timelock | **Solver humano** (scheduler alerta) | decide antes de la ventana | — |

## 12. Depósito anti-abuse (bond)

El depósito anti-abuse es un **VHTLC separado** del escrow de la
operación (~1% del monto del trade). Se usa para disuadir spam y
comportamientos abusivos. El admin del Mostro puede activarlo/desactivarlo
y decidir quién lo paga (`apply_to`: `"take"`, `"make"`, o `"both"`).

### 12.1 Cuándo se cobra

| Escenario | ¿Quién paga bond? | Cuándo |
|-----------|-------------------|--------|
| Taker toma la orden | Taker (buyer o seller según tipo) | Después de `take-buy`/`take-sell` |
| Maker crea la orden | Maker | Antes de publicar la orden en Nostr |
| Both | Maker + Taker | Maker al crear, taker al tomar |

### 12.2 Ciclo de vida del bond VHTLC

```text
                    ┌─────────────────────────────────┐
                    │   Vendedor crea orden (sell)     │
                    └──────────────┬──────────────────┘
                                   │
                    ┌──────────────▼──────────────────┐
                    │  ¿Bond habilitado para makers?   │
                    └──────┬─────────────────┬────────┘
                           │ Sí              │ No
              ┌────────────▼────────┐        │
              │  waiting-maker-bond │        │
              │  (funda bond VHTLC) │        │
              └────────────┬────────┘        │
                           │ bond claimed     │
              ┌────────────▼─────────────────▼────────┐
              │         Orden publicada (pending)      │
              └────────────┬──────────────────────────┘
                           │
                    ┌──────▼──────┐
                    │   Taker toma│
                    └──────┬──────┘
                           │
                    ┌──────▼─────────────────┐
                    │ ¿Bond habilitado para   │
                    │         takers?         │
                    └──────┬─────────────────┘
                           │
              ┌────────────▼────────────┐
              │  waiting-taker-bond     │
              │  (funda bond VHTLC)     │
              └────────────┬────────────┘
                           │ bond claimed
              ┌────────────▼────────────────────┐
              │    Flujo normal de la operación  │
              └─────────────────────────────────┘
```

### 12.3 Salida normal (bond liberado)

En cualquier salida **normal** (completar trade, cancel antes de timeout,
dispute resuelta sin slashing), el bond VHTLC se **libera** de vuelta al
usuario. La wallet del server firma `claim`/`refund` según corresponda.

### 12.4 Slashing del bond

El bond puede ser **slashed** (confiscado) por el server en dos escenarios:

| Escenario | ¿Quién slasha? | ¿A quién van los sats? |
|-----------|----------------|------------------------|
| **Timeout en waiting state** | Scheduler (automático) | Server (o parcialmente al counterparty) |
| **Disputa resuelta por solver** | Solver humano | Server (o parcialmente al counterparty) |

El porcentaje de slashing es configurable: `bond_slash_node_share_pct`
(porcentaje que se queda el node). El resto va al counterparty como
**payout** (se le pide una invoice/VTXO de salida).

### 12.5 Bond VHTLC en Ark

En Lightning, el bond es una **hold invoice** independiente del escrow.
En Ark, el bond también es un **VHTLC separado** (distinto
`paymentHash` y `swapPkScript` del escrow de la operación).

**Parámetros del bond VHTLC:**
- `sender` = wallet del usuario (maker o taker)
- `receiver` = wallet del server (escrow)
- `server` = server pubkey (co-firma)
- `preimageHash` = hash del preimage del bond
- `refundLocktime` = hereda de `hold_invoice_cltv_delta` (mismo que el
  escrow de la operación)
- Monto = ~1% del trade (configurable por admin, con mínimo)

### 12.6 Estados de bond en la DB

| Estado | Significado |
|--------|-------------|
| `waiting-bond` | Bond creado, esperando que el usuario lo funde |
| `bond-funded` | VHTLC bond fundado, esperando que server lo confirme |
| `bond-claimed` | Bond reclamado por server (trade puede proceder) |
| `bond-slashed` | Bond confiscado (timeout o disputa) |
| `bond-released` | Bond liberado de vuelta al usuario (salida normal) |

### 12.7 Configuración del admin

```toml
[bond]
enabled = true              # habilitar/deshabilitar bonds
apply_to = "both"           # "take", "make", o "both"
bond_amount_pct = 1         # porcentaje del trade amount
slash_on_waiting_timeout = true  # slasha si hay timeout en waiting
bond_slash_node_share_pct = 100  # % que se queda el node (0-100)
bond_payout_claim_window_days = 7  # días para reclamar payout
```

**Nota importante:** El bond es un concepto de **protocolo Mostro**, no
de la capa de asentamiento. En Lightning es una hold invoice; en Ark es
un VHTLC. La lógica del daemon (slash, release, payout) es **idéntica**
independientemente de la capa de settlement.

## 13. Suposiciones registradas

- El daemon tendrá su **propia wallet Arkade** (clave `server` +
  conexión a `arkd`) para co-firmar. Confirmado.
- Alcance inicial: **port completo** (incluye reconciliar el payout del
  buyer; en Ark esto significa retirar `do_payment`, no adaptarlo).
- El fundado del VHTLC se dispara **en la wallet del vendedor (sender)**;
  el daemon (server) aporta el `preimageHash` y vigila/confirma la fundada
  (ver §7.1). La UI de escrow ark→ark **aún no existe** en la wallet y hay
  que construirla.
- La **entrada/salida de Ark vía Lightning se hace con solvers, no con
  Boltz**. **Boltz ya no funciona** para este propósito y queda descartado
  del diseño. El on/off-ramp Ark↔Lightning es responsabilidad de la
  **wallet**, no del daemon; el escrow de Mostro es un **VHTLC
  autocontenido** que no depende de ningún swap externo. Cualquier watcher
  de VHTLC orientado a Boltz (p. ej. el de `ark-client`) **no aplica** a
  este flujo.
- El `server` no firma fuera de los escenarios de la tabla de §11.3.
- La garantía “el vendedor no puede trampear” está soportada por la
  propiedad de `R` + la co-firma server en cada hoja (no por la cantidad
  de firmas por sí).
- La tarifa de **0.5% (0.25% vendedor + 0.25% comprador)** va a una
  **billetera Ark del escrow separada** de la del server (ver §5.1).
  Confirmado.
- **Cualquier parte puede abrir la disputa** (como en el protocolo
  oficial de Mostro): el comprador (si envió fiat y el vendedor no
  libera) o el vendedor (si necesita proteger su escrow). Carga de la
  prueba: el **comprador aporta todas las pruebas de que sí envió**;
  el **vendedor aporta pruebas de que no recibió** o de que el comprador
  no envió fiat. Confirmado.
- La **disputa la resuelve un solver humano** (recopilando pruebas de
  ambos peers) siguiendo el ciclo de Mostro-Lightning; el `scheduler`
  solo vigila el `refundLocktime` y escala/alerta (§11.1–11.2).
  Confirmado.
- Hay **dos timers distintos** (según documentación oficial de Mostro:
  https://mostro.network/protocol/):
  - Oferta sin tomar vence a las 24h (`expiration_hours = 24`, sin escrow).
  - Escrow de una operación iniciada corre `refundLocktime`
    (`hold_invoice_cltv_delta = 144` bloques ≈ 24h, con margen de
    `escrow_deadline_margin_blocks = 24`). Al vencer, el server firma
    refund al vendedor si no hay disputa (§6). Confirmado.

## 14. Pendiente antes de escribir código

### 14.1 Vigilar el estado del vtxo VHTLC (reemplaza a `invoice_subscribe`)

En Mostro-Lightning, `invoice_subscribe` vigila el hash del hold invoice
para saber cuándo se pagó. En Ark **no hay un “pago” que interpretar**; hay
que observar el estado del vtxo VHTLC. El SDK `@arkade-os/swap` expone
exactamente lo necesario (hallazgo verificado en
`node_modules/@arkade-os/swap`):

- **`readLockupFate(indexer, { swapPkScript, paymentHash })`** →
  **`"open" | "claimed" | "returned" | "exited" | "unknown"`**, leído
  fresco del indexer (nunca cache). Es la comprobación más autoritativa.
  - `open` → el escrow sigue vivo (el vendedor fundó y aún no se gastó).
  - `claimed` → el vtxo fue `claim` → liberado al comprador.
  - `returned` → el vtxo fue `refund`/`refundWithoutReceiver` → devuelto
    al vendedor.
  - `exited` / `unknown` → unilateral / estado incierto.
  - `unknown` → probablemente aún no fundó (sigue siendo “no escrow”).
- **`findLockupVtxos(indexer, swapPkScript)`** → el conjunto actual de
  vtxos en el script lockup (tanto spendable como swept-recoverable).
- Cada vtxo expone **`expiresAtHeight` / `expiresAt`** → útil para el
  scheduler/`refundLocktime`.

**Modelo de comunicación:** polling periódico (más simple y robusto para
un backend daemon sin UI) y opcionalmente **SSE push** (`IndexerProvider`
soporta subscripciones). El daemon debería **poller + mapear `LockupFate`
a los estados del escrow** (fundido / claim / refund / expirado), igual
que hoy mapea `invoice_paid`/settle.

### 14.2 Política de `refundLocktime` + scheduler (reloj/avisador)

`refundLocktime` se fija al fundar el vtxo y **no se extiende en vuelo**.
Su valor **hereda de `hold_invoice_cltv_delta`** del `settings.toml` del
nodo (configurable por el administrador del Mostro; default 144 bloques
≈ 24h, algunos nodos hasta 432 ≈ 3 días). El scheduler** vigila y actúa
como reloj/avisador:

- Vigila periódicamente las operaciones en discontinuidad.
- Si un escrow está **cerca de `refundLocktime`** sin disputa → **avisa al
  comprador** de que debe abrir disputa (o al solver si ya hay una), para
  resolver **dentro** de la ventana.
- **Política de emergencia** si la ventana se agota sin resolución humana:
  el **server firma refund** a favor del vendedor (`refund` /
  `refundWithoutReceiver`) para **despejar el escrow on-chain** — los
  sats NO se devuelven solos (§6.2), pero el vencimiento **legitima al
  server** a devolver al vendedor.
- El scheduler **nunca arbitra**; solo decide el *timing* de la
  respuesta por defecto (refund por emergencia) cuando no hay disputa ni
  resolución. Con disputa abierta, el vencimiento **no** dispara refund
  automático: espera al **solver humano** (el escrow queda vivo hasta que
  el refunder lo despeje).

**Ajuste de la fila de la tabla:** “refundLocktime venció sin disputa” → el
server firma refund al vendedor por política de emergencia (no es
automático, requiere co-firma server).

## 15. Estado de implementación y checklist (Fase 1: Ark real)

La rama `feature/vhtlc-escrow` trae el **andamiaje** completo (sección
`[arkade]`, `EscrowMode::Arkade`, contexto, boot sin LND, dispatch de
eventos, flujo de take y tick de cancelación del scheduler), pero todo
corre sobre un **backend stub en memoria** (`src/arkade/backend.rs`):
`create_hold_invoice` solo genera un `preimage`/`hash` con `rand`+`sha2`, y
`settle`/`cancel` solo cambian un estado. **No hay conexión a `arkd`, ni
construcción del script VHTLC, ni firma del server.**

**Fase 1 = sustituir el stub por integración real con `arkd`.**

### 15.1 Decisión de librería (desbloqueada)

SDK oficial de Arkade en Rust: **`ark-rs` 0.11.0** —
<https://github.com/arkade-os/rust-sdk> (crates `ark-core`, `ark-client`,
`ark-grpc`, `ark-script`, `ark-fees`, `ark-bdk-wallet`).

**`ark-core/src/vhtlc.rs` ya implementa el VHTLC completo**, por lo que **no
hay que portar nada desde el JS** (`@arkade-os/swap`):

- `VhtlcOptions { sender, receiver, server, preimage_hash,
  refund_locktime, unilateral_claim_delay, unilateral_refund_delay,
  unilateral_refund_without_receiver_delay }`.
- `VhtlcScript::new(options, network)` con `address()` (→ `ArkAddress`),
  `script_pubkey()`, `taproot_spend_info()`, `get_script_map()` y las hojas
  `claim_script()`, `refund_script()`, `refund_without_receiver_script()`,
  `unilateral_claim_script()`, `unilateral_refund_script()`,
  `unilateral_refund_without_receiver_script()`.

> Nota: `bark-wallet` (Second / `ark-bitcoin/bark`) es **otra**
> implementación del protocolo Ark — **descartada**: el diseño sigue a
> Arkade.

### 15.2 Compatibilidad y dependencias (verificado)

| Dependencia | Mostro (daemon) | `ark-rs` 0.11.0 | Veredicto |
| --- | --- | --- | --- |
| Rust (MSRV) | 1.94.0 | 1.86.0 | ✅ compatible |
| `bitcoin` | `0.32.5` (lock `0.32.102`) | `0.32.7` | ✅ misma serie 0.32 |
| `sqlx` | `0.9.0` | `0.8` (feature `sqlite`) | ⚠️ no activar `sqlite` |
| `tonic` | `0.13` + `0.14` | `0.14` | ✅ (suma peso) |

- **No activar la feature `sqlite` de `ark-client`**: mete `sqlx 0.8` en
  paralelo al `sqlx 0.9` del daemon. El escrow se persiste en la **DB de
  Mostro** con el `sqlx` existente (ver §15.4, persistencia).

```toml
ark-rs = { version = "0.11.0", default-features = false, features = ["client", "grpc", "tls-webpki-roots"] }
```

### 15.3 Checklist Fase 1

Config y claves:

- [x] Añadir las deps Ark al `Cargo.toml` (hoy solo hay `hex` + `sha2`).
- [x] Derivar la clave `server` (`XOnlyPublicKey`) desde `server_key`, o
      leer `server_pubkey` (`ArkadeBackend::server_xonly`).
- [x] Añadir a `[arkade]` los tres delays CSV
      (`unilateral_claim_delay`, `unilateral_refund_delay`,
      `unilateral_refund_without_receiver_delay`).
- [x] `preimage_hash` = **`HASH160(preimage)` = `ripemd160(sha256(preimage))`**
      (`ArkadeBackend::preimage_hash`). No es una convención de Boltz ni de
      Lightning: la hoja `claim` construye `OP_HASH160 <preimage_hash>
      OP_EQUAL OP_VERIFY …`, y `OP_HASH160` **es** `RIPEMD160(SHA256(x))`.
      Fuente: `ark-core/src/vhtlc.rs` (`VhtlcOptions::claim_script`). La
      `preimage` la genera y custodia el server.

Conexión al operador:

- [ ] En `main.rs` (modo arkade) construir el `ArkadeBackend` real:
      wallet del server vía `ark-client`, `connect(arkd_url)`, leer
      `server_info` (pubkey del server, fees, network). Hoy `main.rs` no
      conecta nada.

Backend real (reemplazar el stub en `src/arkade/backend.rs`):

- [ ] `create_hold_invoice` → **el daemon NO construye ni funda el script**
      (§7: eso es la wallet del vendedor). Debe: (a) generar `preimage` +
      `preimage_hash`; (b) **publicar los parámetros del contrato** que la
      wallet del vendedor necesita para construirlo (`server` x-only,
      `refund_locktime`, los tres delays CSV); (c) poder **reconstruir el
      `VhtlcScript`/address** (`ArkadeBackend::vhtlc_script`) a partir de
      `sender`+`receiver`+`preimage_hash` para vigilarlo.
      *Bloqueante de diseño:* la `receiver` (comprador) es una **dirección
      Arkade**, no una clave Nostr; hoy `show_vhtlc_escrow_request` solo
      recibe `buyer_pubkey`/`seller_pubkey` de Nostr. Hay que añadir el
      intercambio de claves/direcciones Ark (mensaje o campo de orden).
- [ ] **Observación de estado del VHTLC** (reemplazo de
      `invoice_subscribe`): transición real a `VhtlcStatus::Funded`/
      `Confirmed` cuando el vendedor funda (ver §14.1). Hoy esos estados
      **nunca** se alcanzan y el trade no puede pasar a `Active`.
      *Pendiente de verificar:* el equivalente Rust de `readLockupFate`
      (eventos de contrato de `ark-client` o el indexer). El watcher VHTLC
      de `ark-client` está orientado a **Boltz** → no aplica (ver §13).
      Nota: `ark-client/src/boltz.rs` es un módulo *dentro del SDK Arkade*;
      no implica dependencia del servicio Boltz.
- [ ] `settle_hold_invoice` → co-firmar la hoja `claim` revelando la
      `preimage`; pagar al `receiver`.
- [ ] `cancel_hold_invoice` → co-firmar la hoja `refund` /
      `refund_without_receiver` hacia el `sender`.
- [ ] `chain_height()` → altura del tip (para el `refund_locktime`).

Persistencia:

- [ ] Nueva migración en `migrations/` + helpers en `src/db.rs` para
      `PendingVhtlc` (id, `preimage`, `preimage_hash`, address,
      `refund_locktime`, status, `order_id`). Hoy vive en memoria y se
      pierde al reiniciar.

Entorno y pruebas:

- [x] Unit: mapeo config → `VhtlcOptions`, derivación de `server`, `HASH160`.
- [ ] **Operador de pruebas sin Docker:** usar el `arkd` **público de
      mutinynet** (`https://mutinynet.arkade.sh`, `/v1/info` responde;
      network `mutinynet`) en lugar del regtest con Docker
      (`ghcr.io/arkade-os/arkd`) — las imágenes están geobloqueadas/son muy
      lentas de bajar. El e2e Docker del SDK no es viable aquí.
- [ ] e2e: fund → observar → `claim`/`refund` contra mutinynet.

### 15.4 Fases posteriores (solo referencia)

- **Fase 2 — Liquidación y disputa:** rama arkade en `release`,
  `admin_settle`, `admin_cancel`, `admin_take_dispute`, `fiat_sent`,
  `dispute`; retirar `do_payment`/`reconcile_inflight_payout` (§8).
- **Fase 3 — Destino del comprador y fees:** acción equivalente a
  `add_invoice` para recoger el receiver del comprador (hoy `take_buy` pasa
  `None`) y reparto de la tarifa a `escrow_fee_wallet` (§5.1).
- **Fase 4 — Scheduler de escrow:** reloj de `refundLocktime`
  (avisos + escalado + refund de emergencia) además del tick de
  waiting-state actual (§14.2).
- **Fase 5 — Infra:** persistencia completa, gRPC admin en modo arkade
  (hoy no arranca), wizard de config y bonds (§12).