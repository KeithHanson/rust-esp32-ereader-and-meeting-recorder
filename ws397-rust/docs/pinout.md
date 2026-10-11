# Verified pinout — Waveshare ESP32-S3-ePaper-3.97

All entries cite source file:line in the pinned vendor checkout (`vendor/`,
commit `9b12d40731a80213b927ee8a421cae4082952819`) unless marked otherwise.

## E-paper (SSD1677, GDEM0397T81)

Source: `ESP-IDF/01_E-Paper_Example/components/epaper_port/epaper_port.h:12-20`,
SPI setup in `epaper_port.c:47-57`.

| Signal | GPIO | Notes |
|---|---:|---|
| BUSY | 3 | input; vendor `EPD_ReadBusy` polls `while (ReadBusy == 1)` → **HIGH = busy** (`epaper_port.c:164-175`) |
| DC | 9 | |
| CS | 10 | software CS (`spics_io_num = -1`, `epaper_port.c:49`) |
| SCLK | 11 | |
| MOSI | 12 | |
| RST | 46 | boot-strapping pin — keep vendor reset timing (`epaper_port.c:102-110`) |

SPI: mode 0, 20 MHz, host `SPI3_HOST`, MOSI only (`miso_io_num = -1`), software
CS via GPIO10 (`spics_io_num = -1`, `epaper_port.h:15`, `epaper_port.c:35-61`),
DMA `SPI_DMA_CH_AUTO`, queue 1, `max_transfer_sz` 65536, 1 KiB polled chunks.
Geometry: 800×480 mono, 48000 B/frame; 4-gray 96000 B.
No display power-enable GPIO. Reset sequence and BUSY polarity documented in
`epaper_port.c` (RST pulse 50/2/50 ms verified at `:102-110`; BUSY **HIGH = busy**
per `EPD_ReadBusy` at `:164-175`).

## Unverified — do not use until extracted from schematic / other examples

- SDMMC pins (`ESP-IDF/04_SD_Test/`)
- I²S / ES8311 / NS4150B (`02_Mic_test`, `03_Music`)
- I²C bus + addresses for PCF85063 RTC (`06_I2C_PCF85063`), SHTC3 (`07_I2C_SHTC3`), QMI8658 (`05_QMI8658A`), ES8311, TG28
- Buttons (Up/Function/Down), PMIC behavior
- Flash size / PSRAM mode at runtime → confirm with M1 boot banner
  (`sdkconfig.defaults` currently assumes 16 MB flash + octal PSRAM per the
  product page; M1 must validate).
