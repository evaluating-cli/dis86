# INT 21h AH=44h AL=00h — IOCTL Get Device Information

This note summarizes observed DOS IOCTL return values. Device-driver-specific
bits can vary by DOS implementation; the table describes the values relevant
to the tested environment, not a universal driver ABI.

**Input:** BX = file handle
**Output:** CF clear → DX = AX = device info word; CF set → AX = error code

## Device Information Word (DX) Bit Layout

Bit 7 is the discriminator: 1 = character device, 0 = file/block device.

### Character Device (bit 7 = 1)

| Bit | Hex    | Name              | Meaning                                    |
|-----|--------|-------------------|--------------------------------------------|
| 0   | 0x0001 | `StdIn`           | Is standard input device                  |
| 1   | 0x0002 | `StdOut`          | Is standard output device                 |
| 2   | 0x0004 | `Nul`             | Is NUL device                              |
| 3   | 0x0008 | `Clock`           | Is CLOCK$ device                           |
| 4   | 0x0010 | `Special`         | Supports INT 29h fast output               |
| 5   | 0x0020 | `Binary`          | Binary/raw mode (0 = ASCII/cooked)         |
| 6   | 0x0040 | `EofOnInput`      | No input available / EOF on next read      |
| 7   | 0x0080 | `Device`          | **Is a character device** (always set)     |
| 11  | 0x0800 | `OpenCloseSupport`| Supports open/close (removable media)      |
| 13  | 0x2000 | `OutputUntilBusy` | Output until busy                          |
| 14  | 0x4000 | `IoctlSupport`    | Supports IOCTL channels (AL=02h/03h)       |
| 15  | 0x8000 | —                 | **Always set by IOCTL handler** (mirrors bit 7 into DH, real DOS behavior) |

### File / Block Device (bit 7 = 0)

| Bit  | Hex    | Name           | Meaning                                  |
|------|--------|----------------|------------------------------------------|
| 0-4  | 0x001F | —              | **Drive number** (0=A:, 1=B:, 2=C: …)  |
| 6    | 0x0040 | `NotWritten`   | File has not been written since open     |
| 11   | 0x0800 | `NotRemovable` | Drive is not removable                   |
| 14   | 0x4000 | `NoTimeUpdate` | Don't update timestamp on close          |
| 15   | 0x8000 | `Remote`       | File is on remote/network drive          |

## Common Return Values

| Handle / Device        | DX     | Notes                                                         |
|------------------------|--------|---------------------------------------------------------------|
| CON, input waiting     | 0x80D3 | bits 15,7,6,4,1,0 — char dev + EofOnInput + special + stdin + stdout |
| CON, no input          | 0x8093 | same but bit 6 (EofOnInput) clear                            |
| NUL                    | 0x8084 | bits 15,7,2 — char dev + NUL                                 |
| CD-ROM (MSCD)          | 0xC880 | bits 15,14,11,7 — char dev + IOCTL + open/close + device     |
| File on C: (fresh)     | 0x0042 | bit 6 = NotWritten, bits 0-4 = 2 (C:)                        |
| File on C: (written)   | 0x0002 | bit 6 clear, drive = 2                                        |
| File on A: (read-only) | 0x0040 | NotWritten, drive = 0                                         |

## Notes

- **Bit 15 always set for character devices** — real DOS mirrors DL into DH for
  character device handles, so bit 7 appears in both byte positions of DX.
- **Bit 6 for CON** (`EofOnInput`) is dynamic — set when no keypress is buffered,
  cleared when data is ready. Programs poll this for non-blocking input checks.
- **`reg_ax = reg_dx`** — AX is officially clobbered/set to the same value; programs
  should read DX.
