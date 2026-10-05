/* Additional stock dosdebug command wrappers used by the overlay host. */

static int dosdebug_interrupt_break_command(dosdebug_t *db, uint8_t intno,
                                            int set)
{
    if (!db || !db->connected)
        return -1;
    if (ensure_stopped(db) != 0)
        return -1;

    char cmd[32];
    snprintf(cmd, sizeof(cmd), "%s %02X\n", set ? "bpint" : "bcint",
             (unsigned)intno);
    if (send_cmd(db, cmd) != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;

    size_t n = db->blen - db->bpos;
    const char *response = db->buf + db->bpos;
    int ok = !txt_has(response, n, "Invalid")
          && !txt_has(response, n, "need to be")
          && !txt_has(response, n, "Duplicate BPINT")
          && !txt_has(response, n, "No BPINT");

    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return ok ? 0 : -1;
}

int dosdebug_set_bpint(dosdebug_t *db, uint8_t intno)
{
    return dosdebug_interrupt_break_command(db, intno, 1);
}

int dosdebug_clear_bpint(dosdebug_t *db, uint8_t intno)
{
    return dosdebug_interrupt_break_command(db, intno, 0);
}

int dosdebug_step_into(dosdebug_t *db)
{
    if (!db || !db->connected)
        return -1;
    dosdebug_drain(db);
    return send_cmd(db, "ti\n");
}
