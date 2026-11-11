// Old-school jQuery: DOM ready, $.getJSON, $.ajax with a serialized form,
// .load() of a page fragment, JSONP, delegated events and string templates.
(function ($) {
  "use strict";

  function escape(text) {
    return $("<div>").text(text).html();
  }

  function row(item) {
    return (
      '<tr class="listing" data-id="' + item.id + '">' +
      '<td class="title"><a href="/item/' + item.id + '">' + escape(item.title) + "</a></td>" +
      '<td class="price">$' + item.price.toFixed(2) + "</td>" +
      '<td class="posted">' + escape(item.posted) + "</td></tr>"
    );
  }

  function renderListings(data) {
    var $body = $("#listings tbody");
    $body.find(".placeholder").remove();
    $.each(data.items, function (_, item) {
      $body.append(row(item));
    });
    $("#listings tr.listing:odd").addClass("alt");
    if (data.more) {
      $("#pager").html('<a href="#" class="more" data-page="' + data.more + '">More</a>');
    } else {
      $("#pager").empty().append($("<span/>", { "class": "end", text: "No more listings" }));
    }
  }

  window.renderWeather = function (weather) {
    $("#weather").text(weather.city + ": " + weather.temp + "°");
  };

  $(document).ready(function () {
    $.ajaxSetup({ cache: true });

    $.getJSON("api/listings.json", $("#filters").serialize()).done(renderListings).fail(function (_, status) {
      $("#listings tbody").html('<tr><td colspan="3">Failed: ' + status + "</td></tr>");
    });

    $("#sidebar").load("partials/sidebar.html #links", function (_, status) {
      $(this).attr("data-status", status);
    });

    $.ajax({
      url: "api/weather.js",
      dataType: "jsonp",
      jsonpCallback: "renderWeather",
    });

    $.ajax({
      url: "api/stats.json",
      type: "POST",
      data: { view: "listings" },
      dataType: "json",
    }).then(function (stats) {
      $("#stats").text(stats.views + " views").show();
    });

    $("#pager").on("click", "a.more", function (event) {
      event.preventDefault();
      var page = $(this).data("page");
      $.get("api/listings-" + page + ".json", renderListings, "json");
    });
  });
})(jQuery);
