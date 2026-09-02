// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.DayOfWeek;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * A span of whole days, inclusive at both ends, written
 * {@code "2026-09-01:2026-09-03"}.
 *
 * <p>The argument to {@code get_usage_report} and friends. Both ends are
 * <b>inclusive</b> as dates, but {@link #endTime} is the following midnight, so
 * as an instant range it is half-open - which is what makes a day's usage
 * belong to exactly one day.
 *
 * <p>A single date is a one-day range: {@code "2026-09-01"} parses as
 * {@code 2026-09-01:2026-09-01}. The named forms ({@code today},
 * {@code this_month}, ...) parse too, and are resolved against the <b>local</b>
 * clock of whoever parsed them, so a range named on one side of the network is
 * not necessarily the range the other side would name.
 *
 * <p>The span is capped at five years. A range is not just two dates - the
 * report types aggregate per day over it, so the span bounds how much work one
 * instruction can ask an agent to do.
 */
public record DateRange(LocalDate startDate, LocalDate endDate) implements OpenPortalType {

    /** Five leap-safe years, as {@code MAX_DATE_RANGE_DAYS}. */
    public static final long MAX_DAYS = 5L * 366L;

    public DateRange {
        if (startDate == null || endDate == null) {
            throw new IllegalArgumentException("a date range needs two dates");
        }

        // Swapped rather than refused, matching `DateRange::from_chrono`.
        if (startDate.isAfter(endDate)) {
            LocalDate swap = startDate;
            startDate = endDate;
            endDate = swap;
        }
    }

    public static DateRange of(LocalDate start, LocalDate end) {
        return new DateRange(start, end);
    }

    /**
     * Parse the {@code "<start>:<end>"} form, a single date, or a named range.
     *
     * <p>The names are {@code yesterday}, {@code today}, {@code tomorrow},
     * {@code this_day}, {@code this_week}, {@code last_week},
     * {@code this_month}, {@code last_month}, {@code this_year} and
     * {@code last_year}. Note the two that are <i>not</i> accepted here even
     * though the constructors exist: {@code next_week} and {@code next_month}.
     */
    public static DateRange parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Invalid DateRange - cannot be empty");
        }

        String text = value.trim().toLowerCase(Locale.ROOT);

        switch (text) {
            case "yesterday": return day(LocalDate.now().minusDays(1));
            case "today": case "this_day": return today();
            case "tomorrow": return day(LocalDate.now().plusDays(1));
            case "this_week": return thisWeek();
            case "last_week": return week(LocalDate.now().minusWeeks(1));
            case "this_month": return thisMonth();
            case "last_month": return month(LocalDate.now().minusMonths(1));
            case "this_year": return thisYear();
            case "last_year": return year(LocalDate.now().minusYears(1));
            default: break;
        }

        String[] parts = text.split(":", -1);
        String start;
        String end;

        if (parts.length == 1) {
            start = parts[0];
            end = parts[0];
        } else if (parts.length == 2) {
            start = parts[0];
            end = parts[1];
        } else {
            throw new IllegalArgumentException("Invalid DateRange - must contain two dates, "
                    + "separated by a colon '" + value + "'");
        }

        LocalDate startDate = Dates.parse(start);
        LocalDate endDate = Dates.parse(end);

        long span = Math.abs(java.time.temporal.ChronoUnit.DAYS.between(startDate, endDate));

        if (span > MAX_DAYS) {
            throw new IllegalArgumentException("Invalid DateRange - span of " + span
                    + " days exceeds the maximum of " + MAX_DAYS + " '" + value + "'");
        }

        return new DateRange(startDate, endDate);
    }

    public static DateRange day(LocalDate date) {
        return new DateRange(date, date);
    }

    public static DateRange today() {
        return day(LocalDate.now());
    }

    public static DateRange yesterday() {
        return day(LocalDate.now().minusDays(1));
    }

    public static DateRange tomorrow() {
        return day(LocalDate.now().plusDays(1));
    }

    /** The Monday-to-Sunday week containing {@code date}. */
    public static DateRange week(LocalDate date) {
        LocalDate monday = date.minusDays(date.getDayOfWeek().getValue() - DayOfWeek.MONDAY.getValue());

        return new DateRange(monday, monday.plusDays(6));
    }

    public static DateRange thisWeek() {
        return week(LocalDate.now());
    }

    public static DateRange lastWeek() {
        return week(LocalDate.now().minusWeeks(1));
    }

    public static DateRange nextWeek() {
        return week(LocalDate.now().plusWeeks(1));
    }

    /** The calendar month containing {@code date}, first to last day. */
    public static DateRange month(LocalDate date) {
        LocalDate first = date.withDayOfMonth(1);

        return new DateRange(first, first.plusMonths(1).minusDays(1));
    }

    public static DateRange thisMonth() {
        return month(LocalDate.now());
    }

    public static DateRange lastMonth() {
        return month(LocalDate.now().minusMonths(1));
    }

    public static DateRange nextMonth() {
        return month(LocalDate.now().plusMonths(1));
    }

    /** The calendar year containing {@code date}. */
    public static DateRange year(LocalDate date) {
        LocalDate first = date.withDayOfYear(1);

        return new DateRange(first, first.plusYears(1).minusDays(1));
    }

    public static DateRange thisYear() {
        return year(LocalDate.now());
    }

    public static DateRange lastYear() {
        return year(LocalDate.now().minusYears(1));
    }

    public static DateRange nextYear() {
        return year(LocalDate.now().plusYears(1));
    }

    /** Midnight at the start of the first day, inclusive. */
    public LocalDateTime startTime() {
        return startDate.atStartOfDay();
    }

    /** Midnight at the start of the day <i>after</i> the last - exclusive. */
    public LocalDateTime endTime() {
        return endDate.plusDays(1).atStartOfDay();
    }

    /** Whether a date falls inside the range, both ends included. */
    public boolean contains(LocalDate date) {
        return !date.isBefore(startDate) && !date.isAfter(endDate);
    }

    /** Every day in the range, oldest first. */
    public List<LocalDate> days() {
        List<LocalDate> days = new ArrayList<>();

        for (LocalDate day = startDate; !day.isAfter(endDate); day = day.plusDays(1)) {
            days.add(day);
        }

        return days;
    }

    /**
     * The weeks the range touches, whole.
     *
     * <p>Each entry is a full Monday-to-Sunday week, so the first and last may
     * extend beyond this range. The same is true of {@link #months} and
     * {@link #years}.
     */
    public List<DateRange> weeks() {
        return periods(week(startDate), DateRange::week);
    }

    /** The calendar months the range touches, whole. */
    public List<DateRange> months() {
        return periods(month(startDate), DateRange::month);
    }

    /** The calendar years the range touches, whole. */
    public List<DateRange> years() {
        return periods(year(startDate), DateRange::year);
    }

    private List<DateRange> periods(DateRange first,
            java.util.function.Function<LocalDate, DateRange> containing) {
        List<DateRange> periods = new ArrayList<>();
        DateRange period = first;

        while (!period.startDate.isAfter(endDate)) {
            periods.add(period);

            LocalDate next = period.endDate.plusDays(1);

            if (next.isAfter(endDate)) {
                break;
            }

            period = containing.apply(next);
        }

        return periods;
    }

    @Override
    public String typeName() {
        return "DateRange";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(toString());
    }

    public static DateRange fromJson(JsonNode node) {
        return parse(node.asText());
    }

    @Override
    public String toString() {
        return startDate + ":" + endDate;
    }
}
